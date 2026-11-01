//! The connection thread: ticket, one websocket, reconnects, library sync and
//! song starts. Nothing here runs on the game loop; the game only sends
//! [`Command`]s down a channel.

use super::Status;
use super::protocol::{Incoming, Outgoing, PROTOCOL};
use log::{debug, info, warn};
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

pub(super) enum Command {
    SyncLibrary,
    Started {
        song_id: String,
        chart_hash: String,
        at: Instant,
    },
    Ended {
        song_id: String,
        at: Instant,
    },
    Shutdown,
}

/// Where to connect: the ticket call's answer, turned into a URL.
pub(super) struct Target {
    pub url: String,
    pub page_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TargetError {
    /// Requestline is off or there is no key; the status says which.
    Off(Status),
    BadKey,
    NoPage,
    Unreachable,
}

/// The library as it would be uploaded.
pub(super) struct LocalLibrary {
    pub json: Arc<Vec<u8>>,
    pub hash: String,
}

/// PUTs gzipped bytes to a presigned URL; returns the HTTP status.
pub(super) type Uploader = Arc<dyn Fn(&str, Vec<u8>) -> Result<u16, String> + Send + Sync>;

/// What the thread needs from the outside world. Production asks ArrowCloud
/// and the song cache; tests hand back a local URL and a fixed library.
pub(super) trait Env: Send + 'static {
    fn target(&mut self) -> Result<Target, TargetError>;
    fn library(&mut self) -> Option<LocalLibrary>;
    fn uploader(&self) -> Uploader;
}

#[derive(Clone, Debug)]
pub(super) struct Timing {
    pub backoff: Vec<Duration>,
    pub normal_reconnect: Duration,
    pub stable_after: Duration,
    pub ping_every: Duration,
    pub read_wait: Duration,
    pub link_timeout: Duration,
    pub update_timeout: Duration,
    pub start_max_age: Duration,
    pub close_wait: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            backoff: [2, 5, 15, 30, 60].map(Duration::from_secs).to_vec(),
            normal_reconnect: Duration::from_secs(2),
            stable_after: Duration::from_secs(60),
            // API Gateway closes a websocket idle for 10 minutes.
            ping_every: Duration::from_secs(4 * 60),
            read_wait: Duration::from_millis(50),
            link_timeout: Duration::from_secs(30),
            // The server checks large libraries before answering.
            update_timeout: Duration::from_secs(120),
            start_max_age: Duration::from_secs(120),
            close_wait: Duration::from_secs(1),
        }
    }
}

/// The status this thread writes, ignored once the thread was replaced or
/// told to stop, so a slow old thread can't overwrite a new one's status.
#[derive(Clone)]
pub(super) struct Shared {
    pub status: Arc<Mutex<Status>>,
    pub alive: Arc<AtomicBool>,
}

impl Shared {
    fn alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn set(&self, status: Status) {
        if !self.alive() {
            return;
        }
        if let Ok(mut current) = self.status.lock() {
            *current = status;
        }
    }

    fn set_open(&self, open: bool) {
        if !self.alive() {
            return;
        }
        if let Ok(mut current) = self.status.lock()
            && let Status::Linked { requests_open, .. } = &mut *current
        {
            *requests_open = open;
        }
    }
}

pub(super) struct Handle {
    pub tx: mpsc::Sender<Command>,
    pub alive: Arc<AtomicBool>,
    pub join: thread::JoinHandle<()>,
}

impl Handle {
    pub(super) fn is_running(&self) -> bool {
        !self.join.is_finished() && self.alive.load(Ordering::Acquire)
    }

    /// Asks the thread to close the socket and exit. Doesn't wait.
    pub(super) fn stop(&self) {
        self.alive.store(false, Ordering::Release);
        let _ = self.tx.send(Command::Shutdown);
    }
}

pub(super) fn spawn<E: Env>(
    env: E,
    status: Arc<Mutex<Status>>,
    timing: Timing,
) -> std::io::Result<Handle> {
    let (tx, rx) = mpsc::channel();
    let alive = Arc::new(AtomicBool::new(true));
    let shared = Shared {
        status,
        alive: alive.clone(),
    };
    let join = thread::Builder::new()
        .name("requestline".into())
        .spawn(move || run(env, &rx, &shared, &timing))?;
    Ok(Handle { tx, alive, join })
}

struct PendingStart {
    song_id: String,
    chart_hash: String,
    at: Instant,
}

struct PendingEnd {
    song_id: String,
    at: Instant,
}

/// State kept across reconnects for the life of the thread.
struct Session {
    pending_start: Option<PendingStart>,
    pending_end: Option<PendingEnd>,
    /// Hash the server confirmed this run, so a repeat check is free.
    synced_hash: Option<String>,
    /// A hash the server refused: try again next launch, not every visit.
    rejected_hash: Option<String>,
}

enum SessionEnd {
    Shutdown,
    Upgrade,
    NoPage,
    /// Reconnect after a fresh ticket. `clean` = the server closed normally.
    Retry {
        up_for: Duration,
        clean: bool,
    },
}

fn run<E: Env>(mut env: E, rx: &Receiver<Command>, shared: &Shared, timing: &Timing) {
    let mut session = Session {
        pending_start: None,
        pending_end: None,
        synced_hash: None,
        rejected_hash: None,
    };
    let mut attempt = 0usize;
    while shared.alive() {
        let delay = match env.target() {
            Err(TargetError::Off(status)) => {
                shared.set(status);
                return;
            }
            Err(TargetError::BadKey) => {
                shared.set(Status::BadKey);
                return;
            }
            Err(TargetError::NoPage) => {
                shared.set(Status::NotSetUp);
                return;
            }
            Err(TargetError::Unreachable) => {
                shared.set(Status::Unreachable);
                next_backoff(timing, &mut attempt)
            }
            Ok(target) => match run_socket(&mut env, &target, rx, shared, timing, &mut session) {
                SessionEnd::Shutdown => return,
                SessionEnd::Upgrade => {
                    shared.set(Status::Upgrade);
                    return;
                }
                SessionEnd::NoPage => {
                    shared.set(Status::NotSetUp);
                    return;
                }
                SessionEnd::Retry { up_for, clean } => {
                    shared.set(Status::Unreachable);
                    if up_for >= timing.stable_after {
                        attempt = 0;
                    }
                    if clean {
                        attempt = 0;
                        timing.normal_reconnect
                    } else {
                        next_backoff(timing, &mut attempt)
                    }
                }
            },
        };
        if !wait(rx, delay, &mut session) {
            return;
        }
    }
}

fn next_backoff(timing: &Timing, attempt: &mut usize) -> Duration {
    let delay = timing
        .backoff
        .get(*attempt)
        .or_else(|| timing.backoff.last())
        .copied()
        .unwrap_or(Duration::from_secs(60));
    *attempt += 1;
    delay
}

/// Waits between attempts while still taking commands. `false` = shut down.
fn wait(rx: &Receiver<Command>, delay: Duration, session: &mut Session) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        match rx.recv_timeout(left) {
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => return false,
            Ok(command) => keep_offline(command, session),
            Err(RecvTimeoutError::Timeout) => return true,
        }
    }
}

/// While offline only the latest song start and end are worth keeping; a
/// library check happens anyway after the next `hello`.
fn keep_offline(command: Command, session: &mut Session) {
    match command {
        Command::Started {
            song_id,
            chart_hash,
            at,
        } => {
            session.pending_start = Some(PendingStart {
                song_id,
                chart_hash,
                at,
            });
        }
        Command::Ended { song_id, at } => {
            session.pending_end = Some(PendingEnd { song_id, at });
        }
        Command::SyncLibrary | Command::Shutdown => {}
    }
}

enum SyncState {
    Idle,
    WaitingLink {
        gz: Vec<u8>,
        hash: String,
        since: Instant,
    },
    Uploading {
        upload_id: String,
        hash: String,
        done: Receiver<Result<u16, String>>,
    },
    WaitingUpdate {
        upload_id: String,
        hash: String,
        since: Instant,
    },
}

struct Live<'a, E: Env> {
    env: &'a mut E,
    socket: Socket,
    shared: &'a Shared,
    timing: &'a Timing,
    session: &'a mut Session,
    page: String,
    said_hello: bool,
    server_hash: Option<String>,
    sync: SyncState,
}

fn connect(url: &str, read_wait: Duration) -> Result<Socket, SessionEnd> {
    let retry = SessionEnd::Retry {
        up_for: Duration::ZERO,
        clean: false,
    };
    let (mut socket, _) = match tungstenite::connect(url) {
        Ok(ok) => ok,
        Err(tungstenite::Error::Http(response)) => {
            return match response.status().as_u16() {
                403 => Err(SessionEnd::NoPage),
                // 401: the ticket was used or expired. A new one comes next.
                status => {
                    warn!("requestline: websocket handshake answered {status}");
                    Err(retry)
                }
            };
        }
        Err(error) => {
            warn!("requestline: could not connect: {error}");
            return Err(retry);
        }
    };
    // Blocking socket with a short read timeout: reads give the loop a turn
    // every `read_wait`, writes go out whole (works the same for TLS).
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => Some(&*stream),
        MaybeTlsStream::Rustls(stream) => Some(stream.get_ref()),
        _ => None,
    };
    if let Some(stream) = stream
        && let Err(error) = stream.set_read_timeout(Some(read_wait))
    {
        warn!("requestline: could not set the socket timeout: {error}");
        return Err(retry);
    }
    Ok(socket)
}

fn run_socket<E: Env>(
    env: &mut E,
    target: &Target,
    rx: &Receiver<Command>,
    shared: &Shared,
    timing: &Timing,
    session: &mut Session,
) -> SessionEnd {
    shared.set(Status::Checking);
    let socket = match connect(&target.url, timing.read_wait) {
        Ok(socket) => socket,
        Err(end) => return end,
    };
    let connected_at = Instant::now();
    let mut live = Live {
        env,
        socket,
        shared,
        timing,
        session,
        page: target.page_name.clone().unwrap_or_default(),
        said_hello: false,
        server_hash: None,
        sync: SyncState::Idle,
    };
    let retry = |clean| SessionEnd::Retry {
        up_for: connected_at.elapsed(),
        clean,
    };
    if live.send(&Outgoing::Hello { protocol: PROTOCOL }).is_err() {
        return retry(false);
    }
    let mut last_ping = Instant::now();
    loop {
        // Read everything waiting; the first empty read takes `read_wait`.
        loop {
            match read_text(&mut live.socket) {
                Ok(Some(text)) => {
                    if let Some(end) = live.handle_text(&text) {
                        live.close();
                        return end;
                    }
                }
                Ok(None) => break,
                Err(clean) => {
                    debug!("requestline: socket closed");
                    return retry(clean);
                }
            }
        }
        loop {
            match rx.try_recv() {
                Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => {
                    live.close();
                    return SessionEnd::Shutdown;
                }
                Ok(command) => {
                    if live.handle_command(command).is_err() {
                        return retry(false);
                    }
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if !live.shared.alive() {
            live.close();
            return SessionEnd::Shutdown;
        }
        if live.step_sync().is_err() {
            return retry(false);
        }
        if last_ping.elapsed() >= timing.ping_every {
            if live.send(&Outgoing::Ping).is_err() {
                return retry(false);
            }
            last_ping = Instant::now();
        }
    }
}

/// `Ok(None)` = nothing waiting. `Err(clean)` = the socket is gone.
fn read_text(socket: &mut Socket) -> Result<Option<String>, bool> {
    match socket.read() {
        Ok(Message::Text(text)) => Ok(Some(text.to_string())),
        Ok(Message::Close(_)) => Err(true),
        Ok(_) => Ok(Some(String::new())),
        Err(tungstenite::Error::Io(error))
            if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
        {
            Ok(None)
        }
        Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => Err(true),
        Err(error) => {
            debug!("requestline: read failed: {error}");
            Err(false)
        }
    }
}

impl<E: Env> Live<'_, E> {
    fn send(&mut self, message: &Outgoing) -> Result<(), ()> {
        self.socket
            .send(Message::Text(message.to_text().into()))
            .map_err(|error| warn!("requestline: send failed: {error}"))
    }

    /// Sends a close frame and waits briefly for the server's answer, so the
    /// page sees the game leave right away.
    fn close(&mut self) {
        if self.socket.close(None).is_err() {
            return;
        }
        let deadline = Instant::now() + self.timing.close_wait;
        while Instant::now() < deadline {
            match self.socket.read() {
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => break,
            }
        }
    }

    /// `Some(end)` when the server said this connection should stop.
    fn handle_text(&mut self, text: &str) -> Option<SessionEnd> {
        if text.is_empty() {
            return None;
        }
        let Some(message) = Incoming::parse(text) else {
            debug!("requestline: unreadable message ignored");
            return None;
        };
        match message {
            Incoming::Hello {
                page_name,
                library_hash,
            } => {
                if let Some(page) = page_name {
                    self.page = page;
                }
                self.said_hello = true;
                self.server_hash = library_hash;
                self.shared.set(Status::Linked {
                    page: self.page.clone(),
                    requests_open: false,
                });
                info!("requestline: connected to /{}", self.page);
                // The start first, then the end, so a song that ended while
                // offline still shows as last played.
                if self.deliver_pending_start().is_err() || self.deliver_pending_end().is_err() {
                    return Some(SessionEnd::Retry {
                        up_for: Duration::ZERO,
                        clean: false,
                    });
                }
                if self.start_sync().is_err() {
                    return Some(SessionEnd::Retry {
                        up_for: Duration::ZERO,
                        clean: false,
                    });
                }
            }
            Incoming::State { open, closed_by } => {
                debug!(
                    "requestline: requests {} (closed by {})",
                    if open { "open" } else { "closed" },
                    closed_by.as_deref().unwrap_or("nobody")
                );
                self.shared.set_open(open);
            }
            Incoming::Pong | Incoming::Unknown => {}
            Incoming::LibraryUpload {
                upload_id,
                url,
                max_bytes,
            } => self.on_upload_link(upload_id, &url, max_bytes),
            Incoming::LibraryUpdated {
                version,
                song_count,
                pack_count,
                skipped,
            } => self.on_library_updated(version.as_deref(), song_count, pack_count, skipped),
            Incoming::Started {
                song_id,
                removed,
                repeat,
            } => {
                let song_id = song_id.unwrap_or_default();
                match removed {
                    Some(removed) => info!(
                        "requestline: {} came off the queue for {}",
                        removed.title.as_deref().unwrap_or(&song_id),
                        removed.name.as_deref().unwrap_or("someone")
                    ),
                    None => info!("requestline: {song_id} was not requested"),
                }
                if repeat {
                    debug!("requestline: {song_id} was a repeat report");
                }
            }
            Incoming::Error {
                code,
                message,
                reason,
                for_type,
                upload_id,
            } => {
                let message = message.unwrap_or_default();
                match code.as_str() {
                    "unsupported-protocol" => {
                        warn!("requestline: server dropped this protocol: {message}");
                        return Some(SessionEnd::Upgrade);
                    }
                    "library-rejected" => self.on_library_rejected(upload_id.as_deref(), &message),
                    _ => warn!(
                        "requestline: server error {code} ({}) for {}: {message}",
                        reason.as_deref().unwrap_or("-"),
                        for_type.as_deref().unwrap_or("-")
                    ),
                }
            }
        }
        None
    }

    fn handle_command(&mut self, command: Command) -> Result<(), ()> {
        match command {
            Command::SyncLibrary => {
                if self.said_hello {
                    self.start_sync()?;
                }
            }
            Command::Started {
                song_id,
                chart_hash,
                at,
            } => {
                self.session.pending_start = Some(PendingStart {
                    song_id,
                    chart_hash,
                    at,
                });
                if self.said_hello {
                    self.deliver_pending_start()?;
                }
            }
            Command::Ended { song_id, at } => {
                self.session.pending_end = Some(PendingEnd { song_id, at });
                if self.said_hello {
                    self.deliver_pending_end()?;
                }
            }
            Command::Shutdown => {}
        }
        Ok(())
    }

    fn deliver_pending_start(&mut self) -> Result<(), ()> {
        let Some(start) = self.session.pending_start.take() else {
            return Ok(());
        };
        if start.at.elapsed() > self.timing.start_max_age {
            debug!(
                "requestline: dropped an old song start for {}",
                start.song_id
            );
            return Ok(());
        }
        self.send(&Outgoing::Started {
            song_id: start.song_id,
            chart_hash: start.chart_hash,
        })
    }

    fn deliver_pending_end(&mut self) -> Result<(), ()> {
        let Some(end) = self.session.pending_end.take() else {
            return Ok(());
        };
        if end.at.elapsed() > self.timing.start_max_age {
            debug!("requestline: dropped an old song end for {}", end.song_id);
            return Ok(());
        }
        self.send(&Outgoing::Ended {
            song_id: end.song_id,
        })
    }

    /// Starts a library sync when the local library differs from the server's.
    fn start_sync(&mut self) -> Result<(), ()> {
        if !matches!(self.sync, SyncState::Idle) {
            return Ok(());
        }
        let Some(library) = self.env.library() else {
            debug!("requestline: no songs loaded yet, skipping the library sync");
            return Ok(());
        };
        if self.server_hash.as_deref() == Some(library.hash.as_str())
            || self.session.synced_hash.as_deref() == Some(library.hash.as_str())
        {
            info!("requestline: library unchanged");
            return Ok(());
        }
        if self.session.rejected_hash.as_deref() == Some(library.hash.as_str()) {
            debug!("requestline: this library was refused already; trying again next launch");
            return Ok(());
        }
        let gz = match super::library::gzip(&library.json) {
            Ok(gz) => gz,
            Err(error) => {
                warn!("requestline: could not gzip the library: {error}");
                return Ok(());
            }
        };
        self.send(&Outgoing::LibraryUpload)?;
        self.sync = SyncState::WaitingLink {
            gz,
            hash: library.hash,
            since: Instant::now(),
        };
        Ok(())
    }

    fn on_upload_link(&mut self, upload_id: String, url: &str, max_bytes: Option<u64>) {
        if !matches!(self.sync, SyncState::WaitingLink { .. }) {
            debug!("requestline: upload link nobody asked for, ignored");
            return;
        }
        let SyncState::WaitingLink { gz, hash, .. } =
            std::mem::replace(&mut self.sync, SyncState::Idle)
        else {
            return;
        };
        if let Some(max) = max_bytes
            && gz.len() as u64 > max
        {
            warn!(
                "requestline: library is {} bytes gzipped, over the {max} byte limit",
                gz.len()
            );
            return;
        }
        let (done_tx, done) = mpsc::channel();
        let uploader = self.env.uploader();
        let url = url.to_owned();
        // A separate worker so the socket keeps being read during the PUT.
        let spawned = thread::Builder::new()
            .name("requestline-upload".into())
            .spawn(move || {
                let _ = done_tx.send(uploader(&url, gz));
            });
        match spawned {
            Ok(_) => {
                self.sync = SyncState::Uploading {
                    upload_id,
                    hash,
                    done,
                };
            }
            Err(error) => warn!("requestline: could not start the upload: {error}"),
        }
    }

    fn step_sync(&mut self) -> Result<(), ()> {
        match &self.sync {
            SyncState::Idle => {}
            SyncState::WaitingLink { since, .. } => {
                if since.elapsed() > self.timing.link_timeout {
                    warn!("requestline: no upload link from the server; trying later");
                    self.sync = SyncState::Idle;
                }
            }
            SyncState::Uploading { done, .. } => {
                let result = match done.try_recv() {
                    Ok(result) => result,
                    Err(TryRecvError::Empty) => return Ok(()),
                    Err(TryRecvError::Disconnected) => Err("upload worker ended".to_owned()),
                };
                let SyncState::Uploading {
                    upload_id, hash, ..
                } = std::mem::replace(&mut self.sync, SyncState::Idle)
                else {
                    return Ok(());
                };
                match result {
                    Ok(200..=299) => {
                        self.send(&Outgoing::LibraryUploaded {
                            upload_id: upload_id.clone(),
                            hash: hash.clone(),
                        })?;
                        self.sync = SyncState::WaitingUpdate {
                            upload_id,
                            hash,
                            since: Instant::now(),
                        };
                    }
                    Ok(403) => {
                        warn!("requestline: upload link expired; trying again next check");
                    }
                    Ok(status) => warn!("requestline: library upload answered {status}"),
                    Err(error) => warn!("requestline: library upload failed: {error}"),
                }
            }
            SyncState::WaitingUpdate { since, .. } => {
                if since.elapsed() > self.timing.update_timeout {
                    warn!("requestline: the server didn't confirm the library; trying later");
                    self.sync = SyncState::Idle;
                }
            }
        }
        Ok(())
    }

    fn on_library_updated(
        &mut self,
        version: Option<&str>,
        song_count: u64,
        pack_count: u64,
        skipped: u64,
    ) {
        let SyncState::WaitingUpdate {
            upload_id, hash, ..
        } = &self.sync
        else {
            debug!("requestline: library updated elsewhere ({song_count} songs)");
            return;
        };
        // `version` is the upload id of the sync it confirms.
        if version.is_some_and(|version| version != upload_id) {
            debug!("requestline: library updated by another sync");
            return;
        }
        info!("requestline: synced {song_count} songs in {pack_count} packs ({skipped} skipped)");
        self.session.synced_hash = Some(hash.clone());
        self.server_hash = Some(hash.clone());
        self.sync = SyncState::Idle;
    }

    fn on_library_rejected(&mut self, upload_id: Option<&str>, message: &str) {
        let hash = match &self.sync {
            SyncState::WaitingUpdate {
                upload_id: ours,
                hash,
                ..
            }
            | SyncState::Uploading {
                upload_id: ours,
                hash,
                ..
            } if upload_id.is_none_or(|id| id == ours) => hash.clone(),
            _ => {
                warn!("requestline: library rejected: {message}");
                return;
            }
        };
        warn!("requestline: library rejected: {message}");
        self.session.rejected_hash = Some(hash);
        self.sync = SyncState::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use tungstenite::accept;

    struct TestEnv {
        url: String,
        library: Option<(Vec<u8>, String)>,
        puts: mpsc::Sender<(String, Vec<u8>)>,
    }

    impl Env for TestEnv {
        fn target(&mut self) -> Result<Target, TargetError> {
            Ok(Target {
                url: self.url.clone(),
                page_name: Some("Wafles".into()),
            })
        }

        fn library(&mut self) -> Option<LocalLibrary> {
            self.library.as_ref().map(|(json, hash)| LocalLibrary {
                json: Arc::new(json.clone()),
                hash: hash.clone(),
            })
        }

        fn uploader(&self) -> Uploader {
            let puts = Mutex::new(self.puts.clone());
            Arc::new(move |url: &str, bytes: Vec<u8>| {
                let _ = puts.lock().unwrap().send((url.to_owned(), bytes));
                Ok(200)
            })
        }
    }

    fn fast() -> Timing {
        Timing {
            backoff: vec![Duration::from_millis(100)],
            normal_reconnect: Duration::from_millis(100),
            read_wait: Duration::from_millis(10),
            close_wait: Duration::from_millis(200),
            ..Timing::default()
        }
    }

    struct Server {
        listener: TcpListener,
    }

    type ServerSocket = WebSocket<TcpStream>;

    impl Server {
        fn new() -> (Self, String) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("ws://{}/ws", listener.local_addr().unwrap());
            (Self { listener }, url)
        }

        fn accept(&self) -> ServerSocket {
            let (stream, _) = self.listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            accept(stream).unwrap()
        }
    }

    fn recv_json(socket: &mut ServerSocket) -> serde_json::Value {
        loop {
            match socket.read().unwrap() {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                _ => continue,
            }
        }
    }

    fn send(socket: &mut ServerSocket, text: &str) {
        socket.send(Message::Text(text.to_owned().into())).unwrap();
    }

    fn wait_for(status: &Arc<Mutex<Status>>, want: &Status) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if &*status.lock().unwrap() == want {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "status stayed {:?}, wanted {want:?}",
            status.lock().unwrap()
        );
    }

    type Started = (Handle, Arc<Mutex<Status>>, Receiver<(String, Vec<u8>)>);

    fn start(url: String, library: Option<(Vec<u8>, String)>) -> Started {
        let (puts, put_rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::Off));
        let handle = spawn(TestEnv { url, library, puts }, status.clone(), fast()).unwrap();
        (handle, status, put_rx)
    }

    const HELLO_NULL: &str = r#"{"type":"hello","protocol":1,"role":"game","userId":"u","pageName":"Wafles","isOwner":true,"libraryHash":null}"#;
    const STATE_OPEN: &str = r#"{"type":"state","open":true,"closedBy":null,"gameConnected":true,"queue":[],"nowPlaying":null,"rules":{}}"#;

    #[test]
    fn hello_state_started_and_reconnect() {
        let (server, url) = Server::new();
        let (handle, status, _puts) = start(url, None);

        let mut socket = server.accept();
        assert_eq!(
            recv_json(&mut socket),
            serde_json::json!({"type":"hello","protocol":1})
        );
        send(&mut socket, HELLO_NULL);
        send(&mut socket, STATE_OPEN);
        wait_for(
            &status,
            &Status::Linked {
                page: "Wafles".into(),
                requests_open: true,
            },
        );

        handle
            .tx
            .send(Command::Started {
                song_id: "altruism2/Bounce Back".into(),
                chart_hash: "5d8c5de5bce738f4".into(),
                at: Instant::now(),
            })
            .unwrap();
        assert_eq!(
            recv_json(&mut socket),
            serde_json::json!({"type":"started","songId":"altruism2/Bounce Back","chartHash":"5d8c5de5bce738f4"})
        );

        socket.close(None).unwrap();
        let _ = socket.flush();
        drop(socket);
        let mut again = server.accept();
        assert_eq!(
            recv_json(&mut again),
            serde_json::json!({"type":"hello","protocol":1})
        );

        handle.stop();
        handle.join.join().unwrap();
    }

    #[test]
    fn start_while_disconnected_is_sent_after_hello_unless_old() {
        let (server, url) = Server::new();
        let (handle, status, _puts) = start(url, None);

        // Queued before the server even says hello.
        handle
            .tx
            .send(Command::Started {
                song_id: "P/Old".into(),
                chart_hash: "aaaaaaaaaaaaaaaa".into(),
                at: Instant::now() - Duration::from_secs(180),
            })
            .unwrap();
        let mut socket = server.accept();
        recv_json(&mut socket);
        send(&mut socket, HELLO_NULL);
        // The old one is dropped: the next frame is the fresh start below.
        thread::sleep(Duration::from_millis(100));
        handle
            .tx
            .send(Command::Started {
                song_id: "P/New".into(),
                chart_hash: "bbbbbbbbbbbbbbbb".into(),
                at: Instant::now(),
            })
            .unwrap();
        assert_eq!(recv_json(&mut socket)["songId"], "P/New");

        // Disconnect, start a song while down: delivered after the next hello.
        drop(socket);
        wait_for(&status, &Status::Unreachable);
        handle
            .tx
            .send(Command::Started {
                song_id: "P/Offline".into(),
                chart_hash: "cccccccccccccccc".into(),
                at: Instant::now(),
            })
            .unwrap();
        let mut again = server.accept();
        recv_json(&mut again);
        send(&mut again, HELLO_NULL);
        assert_eq!(recv_json(&mut again)["songId"], "P/Offline");

        handle.stop();
        handle.join.join().unwrap();
    }

    #[test]
    fn ended_is_sent_and_kept_in_order_while_offline() {
        let (server, url) = Server::new();
        let (handle, status, _puts) = start(url, None);
        let ended = |song: &str, age: Duration| Command::Ended {
            song_id: song.into(),
            at: Instant::now() - age,
        };

        let mut socket = server.accept();
        recv_json(&mut socket);
        send(&mut socket, HELLO_NULL);
        thread::sleep(Duration::from_millis(50));
        handle.tx.send(ended("P/Live", Duration::ZERO)).unwrap();
        assert_eq!(
            recv_json(&mut socket),
            serde_json::json!({"type":"ended","songId":"P/Live"})
        );

        // Offline: a start, then its end. After hello: started, then ended.
        drop(socket);
        wait_for(&status, &Status::Unreachable);
        handle
            .tx
            .send(Command::Started {
                song_id: "P/Offline".into(),
                chart_hash: "eeeeeeeeeeeeeeee".into(),
                at: Instant::now(),
            })
            .unwrap();
        handle.tx.send(ended("P/Offline", Duration::ZERO)).unwrap();
        let mut again = server.accept();
        recv_json(&mut again);
        send(&mut again, HELLO_NULL);
        assert_eq!(recv_json(&mut again)["type"], "started");
        assert_eq!(
            recv_json(&mut again),
            serde_json::json!({"type":"ended","songId":"P/Offline"})
        );

        // An end older than 2 minutes is dropped.
        drop(again);
        wait_for(&status, &Status::Unreachable);
        handle
            .tx
            .send(ended("P/Stale", Duration::from_secs(180)))
            .unwrap();
        let mut third = server.accept();
        recv_json(&mut third);
        send(&mut third, HELLO_NULL);
        thread::sleep(Duration::from_millis(50));
        handle
            .tx
            .send(Command::Started {
                song_id: "P/Marker".into(),
                chart_hash: "ffffffffffffffff".into(),
                at: Instant::now(),
            })
            .unwrap();
        assert_eq!(recv_json(&mut third)["songId"], "P/Marker");

        handle.stop();
        handle.join.join().unwrap();
    }

    #[test]
    fn library_syncs_when_the_hash_differs() {
        use std::io::Read;
        let json = br#"{"format":1,"songs":[]}"#.to_vec();
        let hash = "sha256:abc".to_owned();
        let (server, url) = Server::new();
        let (handle, _status, puts) = start(url, Some((json.clone(), hash.clone())));

        let mut socket = server.accept();
        recv_json(&mut socket);
        send(
            &mut socket,
            r#"{"type":"hello","protocol":1,"pageName":"Wafles","libraryHash":"sha256:old"}"#,
        );
        assert_eq!(
            recv_json(&mut socket),
            serde_json::json!({"type":"library-upload"})
        );
        send(
            &mut socket,
            r#"{"type":"library-upload","uploadId":"u1","url":"http://127.0.0.1:9/put","expiresIn":300,"maxBytes":10485760}"#,
        );
        let (put_url, body) = puts.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(put_url, "http://127.0.0.1:9/put");
        let mut unzipped = Vec::new();
        flate2::read::GzDecoder::new(&body[..])
            .read_to_end(&mut unzipped)
            .unwrap();
        assert_eq!(unzipped, json);
        assert_eq!(
            recv_json(&mut socket),
            serde_json::json!({"type":"library-uploaded","uploadId":"u1","hash":"sha256:abc"})
        );
        send(
            &mut socket,
            r#"{"type":"library-updated","version":"u1","songCount":0,"packCount":0,"chartCount":0,"skipped":0}"#,
        );

        // A later check with the same library uploads nothing.
        handle.tx.send(Command::SyncLibrary).unwrap();
        handle
            .tx
            .send(Command::Started {
                song_id: "P/Marker".into(),
                chart_hash: "dddddddddddddddd".into(),
                at: Instant::now(),
            })
            .unwrap();
        assert_eq!(recv_json(&mut socket)["type"], "started");

        handle.stop();
        handle.join.join().unwrap();
    }

    #[test]
    fn same_library_hash_sends_no_upload() {
        let (server, url) = Server::new();
        let (handle, _status, puts) = start(url, Some((b"{}".to_vec(), "sha256:same".to_owned())));
        let mut socket = server.accept();
        recv_json(&mut socket);
        send(
            &mut socket,
            r#"{"type":"hello","protocol":1,"pageName":"Wafles","libraryHash":"sha256:same"}"#,
        );
        handle
            .tx
            .send(Command::Started {
                song_id: "P/Marker".into(),
                chart_hash: "dddddddddddddddd".into(),
                at: Instant::now(),
            })
            .unwrap();
        // The first frame after hello is the start, not a library-upload.
        assert_eq!(recv_json(&mut socket)["type"], "started");
        assert!(puts.try_recv().is_err());
        handle.stop();
        handle.join.join().unwrap();
    }

    #[test]
    fn unsupported_protocol_stops_with_upgrade() {
        let (server, url) = Server::new();
        let (handle, status, _puts) = start(url, None);
        let mut socket = server.accept();
        recv_json(&mut socket);
        send(
            &mut socket,
            r#"{"type":"error","code":"unsupported-protocol","message":"old","supported":[2]}"#,
        );
        wait_for(&status, &Status::Upgrade);
        handle.join.join().unwrap();
    }

    #[test]
    fn shutdown_sends_a_close_frame() {
        let (server, url) = Server::new();
        let (handle, _status, _puts) = start(url, None);
        let mut socket = server.accept();
        recv_json(&mut socket);
        send(&mut socket, HELLO_NULL);
        thread::sleep(Duration::from_millis(50));
        handle.stop();
        let mut closed = false;
        for _ in 0..10 {
            match socket.read() {
                Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => {
                    closed = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(closed);
        handle.join.join().unwrap();
    }
}
