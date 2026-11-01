//! requestline: viewers request songs from the streamer's library on the web.
//!
//! The game's part, protocol 1 (see Wafles' "Requestline in DeadSync:
//! implementation guide"):
//!
//! 1. `POST /v1/requestline/ticket` on ArrowCloud with the streamer's API key
//!    gives a single-use, 60 second ticket and the websocket URL.
//! 2. One websocket stays open while DeadSync runs with requestline on. The
//!    page uses it to show the game as connected, and can close requests when
//!    the game leaves and open them again when it comes back.
//! 3. After `hello`, the library is uploaded (gzipped format 1, through a
//!    presigned S3 PUT) only when its hash differs from the server's.
//! 4. Each song start is reported so its first request leaves the queue.
//!
//! Everything runs on the `requestline` thread; the game loop only sends it
//! commands. Set `DEADSYNC_REQUESTLINE_URL` (for example
//! `http://127.0.0.1:8787`) to point the ticket call at another API base.

mod connection;
mod library;
mod protocol;

use connection::{Command, Env, Handle, LocalLibrary, Target, TargetError, Timing, Uploader};
use deadsync_config::requestline::RequestlineSide;
use deadsync_net as network;
use deadsync_profile::{PlayerSide, compat as profile};
use log::warn;
use serde::Deserialize;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

pub use library::song_id;

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
    /// Getting a ticket or connecting.
    Checking,
    Linked {
        page: String,
        requests_open: bool,
    },
    /// The ArrowCloud account has no requestline page yet.
    NotSetUp,
    /// ArrowCloud rejected the key.
    BadKey,
    /// The server dropped the protocol this build speaks.
    Upgrade,
    /// Can't reach the server; reconnecting.
    Unreachable,
}

static STATUS: LazyLock<Arc<Mutex<Status>>> = LazyLock::new(|| Arc::new(Mutex::new(Status::Off)));
static CONNECTION: Mutex<Option<Handle>> = Mutex::new(None);
/// A key ArrowCloud refused. Not retried until the key or the settings change.
static REJECTED_KEY: Mutex<Option<String>> = Mutex::new(None);

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

/// The streamer's ArrowCloud key, or the status that explains why there is
/// none (requestline or ArrowCloud off, or no joined, non-guest profile).
fn api_key() -> Result<String, Status> {
    let cfg = deadsync_config::runtime::get();
    // An ArrowCloud feature: off whenever ArrowCloud itself is off.
    if !cfg.enable_arrowcloud || !cfg.requestline.enabled {
        return Err(Status::Off);
    }
    let side = match cfg.requestline.side {
        RequestlineSide::P1 => PlayerSide::P1,
        RequestlineSide::P2 => PlayerSide::P2,
    };
    if !profile::is_session_side_joined(side) || profile::is_session_side_guest(side) {
        return Err(Status::NoKey);
    }
    let key = profile::get_for_side(side).arrowcloud_api_key;
    let key = key.trim();
    if key.is_empty() {
        return Err(Status::NoKey);
    }
    Ok(key.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ticket {
    ticket: String,
    web_socket_url: String,
    #[serde(default)]
    page_name: Option<String>,
}

/// Maps the ticket call's answer.
fn ticket_from_response(status: u16, body: &str) -> Result<Ticket, TargetError> {
    match status {
        200 => serde_json::from_str(body).map_err(|error| {
            warn!("requestline: unreadable ticket: {error}");
            TargetError::Unreachable
        }),
        401 | 403 => Err(TargetError::BadKey),
        404 => Err(TargetError::NoPage),
        status => {
            warn!(
                "requestline: ticket call answered {status}: {}",
                network::log_body_snippet(body)
            );
            Err(TargetError::Unreachable)
        }
    }
}

fn fetch_ticket(key: &str) -> Result<Ticket, TargetError> {
    let bearer = format!("Bearer {key}");
    let response = network::get_agent()
        .post(&format!("{}/v1/requestline/ticket", base_url()))
        .header("Authorization", &bearer)
        .config()
        .http_status_as_error(false)
        .build()
        .send_empty()
        .map_err(|error| {
            warn!("requestline: could not reach ArrowCloud: {error}");
            TargetError::Unreachable
        })?;
    let status = response.status().as_u16();
    let body = network::read_text_body_or_empty(response);
    ticket_from_response(status, &body)
}

/// Percent-encodes a query value (RFC 3986 unreserved characters pass).
fn encode_query(value: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn socket_url(ticket: &Ticket) -> String {
    let separator = if ticket.web_socket_url.contains('?') {
        '&'
    } else {
        '?'
    };
    format!(
        "{}{separator}ticket={}",
        ticket.web_socket_url,
        encode_query(&ticket.ticket)
    )
}

/// The real world: ArrowCloud for tickets, the song cache for the library.
struct LiveEnv {
    /// `(song cache generation, library)`, so returning to song select
    /// doesn't re-hash every song.
    cached: Option<(u64, Arc<Vec<u8>>, String)>,
}

impl Env for LiveEnv {
    fn target(&mut self) -> Result<Target, TargetError> {
        let key = api_key().map_err(TargetError::Off)?;
        if REJECTED_KEY
            .lock()
            .is_ok_and(|rejected| rejected.as_deref() == Some(key.as_str()))
        {
            return Err(TargetError::BadKey);
        }
        let ticket = fetch_ticket(&key).inspect_err(|error| {
            if *error == TargetError::BadKey
                && let Ok(mut rejected) = REJECTED_KEY.lock()
            {
                *rejected = Some(key.clone());
            }
        })?;
        Ok(Target {
            url: socket_url(&ticket),
            page_name: ticket.page_name,
        })
    }

    fn library(&mut self) -> Option<LocalLibrary> {
        let generation = deadsync_simfile::runtime_cache::song_cache_generation();
        if let Some((cached_generation, json, hash)) = &self.cached
            && *cached_generation == generation
        {
            return Some(LocalLibrary {
                json: json.clone(),
                hash: hash.clone(),
            });
        }
        let built = library::collect_library();
        if built.songs.is_empty() {
            return None;
        }
        let json = Arc::new(library::library_json(&built));
        let hash = library::library_hash(&json);
        self.cached = Some((generation, json.clone(), hash.clone()));
        Some(LocalLibrary { json, hash })
    }

    fn uploader(&self) -> Uploader {
        Arc::new(|url: &str, bytes: Vec<u8>| {
            // Presigned S3 link: no Authorization header, the URL grants access.
            network::get_streaming_agent()
                .put(url)
                .header("Content-Type", "application/gzip")
                .config()
                .http_status_as_error(false)
                .build()
                .send(&bytes[..])
                .map(|response| response.status().as_u16())
                .map_err(|error| error.to_string())
        })
    }
}

/// Starts the connection thread unless it's already running. Never blocks.
fn ensure_running() -> Option<std::sync::mpsc::Sender<Command>> {
    let mut connection = CONNECTION.lock().ok()?;
    if let Some(handle) = connection.as_ref()
        && handle.is_running()
    {
        return Some(handle.tx.clone());
    }
    // Check cheaply on this thread first, so a disabled feature or a missing
    // profile doesn't spawn a thread on every song select.
    if let Err(status) = api_key() {
        set_status(status);
        *connection = None;
        return None;
    }
    set_status(Status::Checking);
    match connection::spawn(LiveEnv { cached: None }, STATUS.clone(), Timing::default()) {
        Ok(handle) => {
            let tx = handle.tx.clone();
            *connection = Some(handle);
            Some(tx)
        }
        Err(error) => {
            warn!("requestline: could not start the connection thread: {error}");
            set_status(Status::Unreachable);
            None
        }
    }
}

fn stop_connection(wait: Duration) {
    let handle = CONNECTION.lock().ok().and_then(|mut c| c.take());
    let Some(handle) = handle else {
        return;
    };
    handle.stop();
    let deadline = Instant::now() + wait;
    while !handle.join.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if handle.join.is_finished() {
        let _ = handle.join.join();
    }
}

/// Entering song select: connect if needed, then make sure the server has
/// this machine's library. Does nothing when requestline is off.
pub fn sync_library_if_needed() {
    if let Some(tx) = ensure_running() {
        let _ = tx.send(Command::SyncLibrary);
    }
}

/// After the requestline settings change: reconnect with the new profile, or
/// stop when turned off.
pub fn on_settings_changed() {
    if let Ok(mut rejected) = REJECTED_KEY.lock() {
        *rejected = None;
    }
    // The old thread closes its socket on its own; don't hold up the game.
    stop_connection(Duration::ZERO);
    // Sets Off / NoKey itself when there's nothing to connect with.
    let _ = ensure_running();
}

/// A song started (not a restart): its first request leaves the queue.
/// Fire and forget; never blocks gameplay.
pub fn on_song_started(simfile_path: &Path, chart_hash: &str) {
    let Some(song_id) = song_id(simfile_path) else {
        return;
    };
    if let Some(tx) = ensure_running() {
        let _ = tx.send(Command::Started {
            song_id,
            chart_hash: chart_hash.trim().to_ascii_lowercase(),
            at: Instant::now(),
        });
    }
}

/// App exit: close the socket cleanly so the page sees the game leave now
/// rather than when API Gateway notices.
pub fn shutdown() {
    stop_connection(Duration::from_millis(1500));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_answers_map_to_outcomes() {
        assert_eq!(
            ticket_from_response(
                200,
                r#"{"ticket":"9f2c","expiresIn":60,"webSocketUrl":"wss://requestline.arrowcloud.dance/ws","pageName":"Wafles"}"#
            ),
            Ok(Ticket {
                ticket: "9f2c".into(),
                web_socket_url: "wss://requestline.arrowcloud.dance/ws".into(),
                page_name: Some("Wafles".into()),
            })
        );
        assert_eq!(
            ticket_from_response(200, "nope"),
            Err(TargetError::Unreachable)
        );
        assert_eq!(
            ticket_from_response(401, r#"{"error":"x"}"#),
            Err(TargetError::BadKey)
        );
        assert_eq!(ticket_from_response(403, ""), Err(TargetError::BadKey));
        assert_eq!(
            ticket_from_response(
                404,
                r#"{"error":"Set up a Requestline page first","code":"no-page","setupUrl":"https://requestline.arrowcloud.dance/setup"}"#
            ),
            Err(TargetError::NoPage)
        );
        for status in [500, 503, 504] {
            assert_eq!(
                ticket_from_response(status, ""),
                Err(TargetError::Unreachable)
            );
        }
    }

    #[test]
    fn socket_url_carries_the_encoded_ticket() {
        let ticket = |t: &str, url: &str| Ticket {
            ticket: t.into(),
            web_socket_url: url.into(),
            page_name: None,
        };
        assert_eq!(
            socket_url(&ticket("3f2a", "wss://requestline.arrowcloud.dance/ws")),
            "wss://requestline.arrowcloud.dance/ws?ticket=3f2a"
        );
        assert_eq!(
            socket_url(&ticket("a b/+", "wss://h/ws?x=1")),
            "wss://h/ws?x=1&ticket=a%20b%2F%2B"
        );
    }
}
