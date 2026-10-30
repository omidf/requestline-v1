//! requestline: viewers request songs from the streamer's library on the web.
//!
//! The game's part is small. It uses the streamer's ArrowCloud key (same
//! routes and auth as score submission), keeps the server's copy of the song
//! list current, and says when a song starts so it comes off the queue.
//! Every call runs on a worker thread; nothing here touches gameplay timing.
//!
//! Set `DEADSYNC_REQUESTLINE_URL` (for example `http://127.0.0.1:8787`) to
//! point at a local test server instead of ArrowCloud.

use deadsync_config::requestline::RequestlineSide;
use deadsync_net as network;
use deadsync_profile::{PlayerSide, compat as profile};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::thread;

const DEFAULT_BASE_URL: &str = "https://api.arrowcloud.dance";
const BASE_URL_ENV: &str = "DEADSYNC_REQUESTLINE_URL";

/// What the options screen can say about requestline right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Turned off (or ArrowCloud is off) in Online Score Services > ArrowCloud Options.
    #[default]
    Off,
    /// On, but the chosen side has no joined, signed-in ArrowCloud profile.
    NoKey,
    Checking,
    Linked {
        page: String,
        requests_open: bool,
    },
    /// The ArrowCloud account has no requestline page yet.
    NotSetUp,
    /// ArrowCloud rejected the key.
    BadKey,
    /// The server is too new for this build.
    Upgrade,
    Unreachable,
}

static STATUS: LazyLock<Mutex<Status>> = LazyLock::new(|| Mutex::new(Status::Off));
static SYNC_RUNNING: AtomicBool = AtomicBool::new(false);
/// `(song cache generation, hash)` of this machine's library, so going back to
/// song select doesn't re-hash every song. The server is still asked each time.
static LOCAL_HASH: LazyLock<Mutex<Option<(u64, String)>>> = LazyLock::new(|| Mutex::new(None));

#[must_use]
pub fn status() -> Status {
    STATUS.lock().map(|s| s.clone()).unwrap_or_default()
}

fn set_status(status: Status) {
    if let Ok(mut current) = STATUS.lock() {
        *current = status;
    }
}

fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned())
}

fn route(path: &str) -> String {
    format!("{}/v1/requestline/{path}", base_url())
}

/// The streamer's ArrowCloud key, or `None` when requestline is off or the
/// chosen side has no joined, non-guest profile with a key.
fn api_key() -> Option<String> {
    let cfg = deadsync_config::runtime::get();
    // An ArrowCloud feature: off whenever ArrowCloud itself is off.
    if !cfg.enable_arrowcloud || !cfg.requestline.enabled {
        set_status(Status::Off);
        return None;
    }
    let side = match cfg.requestline.side {
        RequestlineSide::P1 => PlayerSide::P1,
        RequestlineSide::P2 => PlayerSide::P2,
    };
    if !profile::is_session_side_joined(side) || profile::is_session_side_guest(side) {
        set_status(Status::NoKey);
        return None;
    }
    let key = profile::get_for_side(side).arrowcloud_api_key;
    let key = key.trim();
    if key.is_empty() {
        set_status(Status::NoKey);
        return None;
    }
    Some(key.to_owned())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeResponse {
    page: String,
    #[serde(default)]
    requests_open: bool,
    #[serde(default)]
    library_hash: Option<String>,
}

/// Asks who the key belongs to and records the result in [`status`].
/// Returns the server's library hash when the page exists.
fn fetch_me(key: &str) -> Option<Option<String>> {
    let bearer = format!("Bearer {key}");
    let response = match network::get_agent()
        .get(&route("me"))
        .header("Authorization", &bearer)
        .config()
        .http_status_as_error(false)
        .build()
        .call()
    {
        Ok(response) => response,
        Err(error) => {
            warn!("requestline: could not reach the server: {error}");
            set_status(Status::Unreachable);
            return None;
        }
    };
    match response.status().as_u16() {
        200 => match network::read_json_body::<MeResponse>(response) {
            Ok(me) => {
                set_status(Status::Linked {
                    page: me.page,
                    requests_open: me.requests_open,
                });
                Some(me.library_hash)
            }
            Err(error) => {
                warn!("requestline: unreadable /me response: {error:?}");
                set_status(Status::Unreachable);
                None
            }
        },
        401 | 403 => {
            set_status(Status::BadKey);
            None
        }
        404 => {
            set_status(Status::NotSetUp);
            None
        }
        426 => {
            set_status(Status::Upgrade);
            None
        }
        status => {
            warn!("requestline: /me answered {status}");
            set_status(Status::Unreachable);
            None
        }
    }
}

/// Call after the requestline settings change: check the account and library again.
pub fn on_settings_changed() {
    sync_library_if_needed();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadChart {
    #[serde(rename = "type")]
    chart_type: String,
    difficulty: String,
    meter: u32,
    hash: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadSong {
    song_id: String,
    title: String,
    subtitle: String,
    artist: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_translit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artist_translit: Option<String>,
    pack: String,
    bpm_min: f64,
    bpm_max: f64,
    length_seconds: i32,
    charts: Vec<UploadChart>,
}

#[derive(Serialize)]
struct UploadBody<'a> {
    hash: &'a str,
    songs: &'a [UploadSong],
}

/// `Pack/Song` from a simfile path: the song folder and the folder above it.
#[must_use]
pub fn song_id(simfile_path: &Path) -> Option<String> {
    let song_dir = simfile_path.parent()?;
    let pack_dir = song_dir.parent()?;
    let song = song_dir.file_name()?.to_string_lossy();
    let pack = pack_dir.file_name()?.to_string_lossy();
    Some(format!("{pack}/{song}"))
}

fn differs(value: &str, from: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value != from.trim()).then(|| value.to_owned())
}

fn collect_songs() -> Vec<UploadSong> {
    // Clone the pack list (songs are `Arc`s) so the cache lock is held briefly.
    let packs = deadsync_simfile::runtime_cache::get_song_cache().clone();
    let mut seen = std::collections::HashSet::new();
    let mut songs = Vec::new();
    for pack in &packs {
        for song in &pack.songs {
            let Some(id) = song_id(&song.simfile_path) else {
                continue;
            };
            // Two song roots can hold the same Pack/Song; keep the first, like the wheel.
            if !seen.insert(id.clone()) {
                continue;
            }
            // A blank #TITLE falls back to the song folder name.
            let title = if song.title.trim().is_empty() {
                id.rsplit('/').next().unwrap_or_default().to_owned()
            } else {
                song.title.clone()
            };
            songs.push(UploadSong {
                song_id: id,
                title,
                subtitle: song.subtitle.clone(),
                artist: song.artist.clone(),
                title_translit: differs(&song.translit_title, &song.title),
                artist_translit: differs(&song.translit_artist, &song.artist),
                pack: pack.group_name.clone(),
                bpm_min: song.min_bpm,
                bpm_max: song.max_bpm,
                length_seconds: song.total_length_seconds,
                charts: song
                    .charts
                    .iter()
                    .map(|chart| UploadChart {
                        chart_type: chart.chart_type.clone(),
                        difficulty: chart.difficulty.clone(),
                        meter: chart.meter,
                        hash: chart.short_hash.clone(),
                    })
                    .collect(),
            });
        }
    }
    songs.sort_by(|a, b| a.song_id.cmp(&b.song_id));
    songs
}

/// Hash of exactly what would be uploaded (songs are sorted by id), so any
/// change the server would see, including an edited title, triggers a sync.
fn library_hash(songs: &[UploadSong]) -> String {
    use sha2::{Digest, Sha256};
    let json = serde_json::to_vec(songs).unwrap_or_default();
    Sha256::digest(&json)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadLink {
    upload_id: String,
    upload_url: String,
}

#[derive(Deserialize)]
struct CommitResult {
    songs: u64,
    packs: u64,
}

fn post_json<B: Serialize, T: for<'de> Deserialize<'de>>(
    key: &str,
    path: &str,
    body: &B,
) -> Result<T, String> {
    let bearer = format!("Bearer {key}");
    let response = network::get_agent()
        .post(&route(path))
        .header("Authorization", &bearer)
        .header("Content-Type", "application/json")
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(body)
        .map_err(|error| format!("{path}: {error}"))?;
    let status = response.status().as_u16();
    if status == 200 {
        network::read_json_body(response).map_err(|error| format!("{path}: {error:?}"))
    } else {
        let text = network::read_text_body_or_empty(response);
        Err(format!(
            "{path} answered {status}: {}",
            network::log_body_snippet(text)
        ))
    }
}

fn upload_library(key: &str, hash: &str, songs: &[UploadSong]) -> Result<CommitResult, String> {
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    serde_json::to_writer(&mut gzip, &UploadBody { hash, songs })
        .map_err(|error| format!("encode: {error}"))?;
    gzip.flush().map_err(|error| format!("gzip: {error}"))?;
    let gzipped = gzip.finish().map_err(|error| format!("gzip: {error}"))?;

    #[derive(Serialize)]
    struct UploadRequest<'a> {
        hash: &'a str,
        bytes: usize,
    }
    let link: UploadLink = post_json(
        key,
        "library/upload",
        &UploadRequest {
            hash,
            bytes: gzipped.len(),
        },
    )?;
    // Presigned link: no Authorization header, the URL itself grants access.
    network::get_streaming_agent()
        .put(&link.upload_url)
        .header("Content-Type", "application/gzip")
        .send(&gzipped[..])
        .map_err(|error| format!("upload: {error}"))?;

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct CommitRequest<'a> {
        upload_id: &'a str,
    }
    post_json(
        key,
        "library/commit",
        &CommitRequest {
            upload_id: &link.upload_id,
        },
    )
}

/// Checks the account and, if the server's song list differs from this
/// machine's, uploads it. Runs on every song select visit: one small `me` call
/// on a worker thread, so a library the server lost or reset is caught right
/// away. Does nothing when requestline is off or a check is already running.
pub fn sync_library_if_needed() {
    let Some(key) = api_key() else {
        return;
    };
    if SYNC_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    set_status(Status::Checking);
    let spawned = thread::Builder::new()
        .name("requestline-sync".into())
        .spawn(move || {
            run_sync(&key);
            SYNC_RUNNING.store(false, Ordering::Release);
        });
    if let Err(error) = spawned {
        warn!("requestline: could not start the sync thread: {error}");
        SYNC_RUNNING.store(false, Ordering::Release);
    }
}

fn run_sync(key: &str) {
    let Some(server_hash) = fetch_me(key) else {
        return;
    };
    let generation = deadsync_simfile::runtime_cache::song_cache_generation();
    let cached = LOCAL_HASH.lock().ok().and_then(|local| {
        local
            .as_ref()
            .filter(|(g, _)| *g == generation)
            .map(|(_, h)| h.clone())
    });
    if cached.is_some() && cached == server_hash {
        return;
    }
    let songs = collect_songs();
    if songs.is_empty() {
        debug!("requestline: no songs loaded yet, skipping the library sync");
        return;
    }
    let hash = library_hash(&songs);
    if let Ok(mut local) = LOCAL_HASH.lock() {
        *local = Some((generation, hash.clone()));
    }
    if server_hash.as_deref() != Some(hash.as_str()) {
        match upload_library(key, &hash, &songs) {
            Ok(result) => info!(
                "requestline: synced {} songs in {} packs",
                result.songs, result.packs
            ),
            Err(error) => warn!("requestline: song list didn't sync: {error}"),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartedRequest {
    song_id: String,
    chart_hash: String,
}

#[derive(Deserialize)]
struct StartedResponse {
    #[serde(default)]
    removed: Option<serde_json::Value>,
}

/// Tells the server a song started, so the first request for it comes off the
/// queue. Fire and forget: never blocks gameplay, and a failure only logs.
pub fn on_song_started(simfile_path: &Path, chart_hash: &str) {
    let Some(key) = api_key() else {
        return;
    };
    let Some(song_id) = song_id(simfile_path) else {
        return;
    };
    let body = StartedRequest {
        song_id,
        chart_hash: chart_hash.to_owned(),
    };
    let spawned = thread::Builder::new()
        .name("requestline-started".into())
        .spawn(
            move || match post_json::<_, StartedResponse>(&key, "started", &body) {
                Ok(response) if response.removed.is_some() => {
                    info!("requestline: {} came off the queue", body.song_id);
                }
                Ok(_) => debug!("requestline: {} wasn't requested", body.song_id),
                Err(error) => warn!("requestline: couldn't report the song start: {error}"),
            },
        );
    if let Err(error) = spawned {
        warn!("requestline: could not start the report thread: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, hashes: &[&str]) -> UploadSong {
        UploadSong {
            song_id: id.to_owned(),
            title: String::new(),
            subtitle: String::new(),
            artist: String::new(),
            title_translit: None,
            artist_translit: None,
            pack: String::new(),
            bpm_min: 0.0,
            bpm_max: 0.0,
            length_seconds: 0,
            charts: hashes
                .iter()
                .map(|hash| UploadChart {
                    chart_type: "dance-single".into(),
                    difficulty: "Challenge".into(),
                    meter: 12,
                    hash: (*hash).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn song_id_is_pack_and_song_folder() {
        let path = Path::new("Songs")
            .join("ITL Online 2026")
            .join("Stay")
            .join("Stay.ssc");
        assert_eq!(song_id(&path).as_deref(), Some("ITL Online 2026/Stay"));
        assert_eq!(song_id(Path::new("Stay.ssc")), None);
    }

    #[test]
    fn library_hash_changes_with_anything_the_server_would_see() {
        let a = library_hash(&[song("P/A", &["h1"]), song("P/B", &["h2"])]);
        assert_eq!(
            a,
            library_hash(&[song("P/A", &["h1"]), song("P/B", &["h2"])])
        );
        let mut renamed = song("P/A", &["h1"]);
        renamed.title = "different title".into();
        assert_ne!(a, library_hash(&[renamed, song("P/B", &["h2"])]));
        assert_ne!(
            a,
            library_hash(&[song("P/A", &["h1"]), song("P/B", &["h3"])])
        );
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn translit_is_sent_only_when_it_adds_something() {
        assert_eq!(differs("", "Title"), None);
        assert_eq!(differs("Title", "Title"), None);
        assert_eq!(
            differs("Senbonzakura", "千本桜").as_deref(),
            Some("Senbonzakura")
        );
    }
}
