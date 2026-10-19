//! Everything that talks to the bundled yt-dlp.exe.
//!
//! The installer drops `yt-dlp.exe` next to the app. On first start it is copied to
//! `%LOCALAPPDATA%\Harmony` (always writable) and updated there with `yt-dlp -U`.

use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::{net::TcpStream, process::Command, sync::Mutex as AsyncMutex, time::timeout};

use crate::store;

/// Optional manual override: "firefox", "chrome", "edge", ... ("" = automatic).
/// You can also place a cookies.txt (Netscape format) next to the app or in the data folder.
const COOKIES_BROWSER: &str = "";
/// If YouTube shows its bot check, these browsers are tried in order for a signed-in session.
const AUTO_COOKIE_BROWSERS: [&str; 4] = ["firefox", "chrome", "edge", "brave"];
/// YouTube app "clients" that often work without a sign-in. Which ones do changes over time.
const CLIENT_FALLBACKS: [&str; 3] = ["android_vr", "tv", "web_safari"];
const AUTO_COOKIE_COOLDOWN: Duration = Duration::from_secs(600);
const UPDATE_INTERVAL: Duration = Duration::from_secs(12 * 3600);

static PLAYLIST_REQ: AtomicU64 = AtomicU64::new(0);
static STREAM_REQ: AtomicU64 = AtomicU64::new(0);
/// One stream lookup at a time.
static STREAM_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
/// Held while yt-dlp updates itself so no lookup starts on a half-replaced exe.
static UPDATE_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
static EXE: OnceLock<PathBuf> = OnceLock::new();

/// Ways of reaching YouTube, tried in this order until one returns something:
/// plain, alternate player clients (no sign-in), the imported cookies.txt, browser cookies.
#[derive(Clone, Copy, PartialEq)]
enum Strategy {
    Default,
    Client(&'static str),
    CookieFile,
    Browser(&'static str),
}

struct Memory {
    good: Option<Strategy>, // what worked last time: tried first from then on
    browsers_failed_at: Option<Instant>,
}
static MEMORY: Mutex<Memory> = Mutex::new(Memory {
    good: None,
    browsers_failed_at: None,
});

#[derive(Serialize)]
pub struct Track {
    id: String,
    title: String,
    dur: String,
    secs: u64,
}

#[derive(Serialize)]
pub struct PlaylistResult {
    tracks: Vec<Track>,
    error: String,
}

#[derive(Serialize)]
pub struct StreamResult {
    url: String,
    offline: bool,
    auth_blocked: bool,
    error: String,
}

// ----------------------------------------------------------------- executable

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn exe() -> &'static PathBuf {
    EXE.get_or_init(|| {
        let bundled = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("yt-dlp.exe")))
            .filter(|p| p.exists());
        let local = store::data_dir().join("yt-dlp.exe");
        if let Some(b) = &bundled {
            let newer = match (modified(b), modified(&local)) {
                (Some(b), Some(l)) => b > l,
                (Some(_), None) => true,
                _ => false,
            };
            if newer && fs::copy(b, &local).is_err() {
                return b.clone();
            }
        }
        if local.exists() {
            local
        } else {
            bundled.unwrap_or_else(|| PathBuf::from("yt-dlp"))
        }
    })
}

fn command(args: &[String]) -> Command {
    let mut cmd = Command::new(exe());
    cmd.args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");
    // yt-dlp looks for a JavaScript runtime (the bundled deno.exe) on PATH.
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let mut paths = vec![dir];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

/// Called once at startup: prepares the local copy and updates it (at most every 12 hours).
pub async fn prepare_and_update() {
    let path = exe().clone();
    let data = store::data_dir();
    let marker = data.join("ytdlp.checked");
    let fresh = modified(&marker)
        .and_then(|t| t.elapsed().ok())
        .map_or(false, |age| age < UPDATE_INTERVAL);
    if fresh || !path.starts_with(&data) {
        return;
    }

    let _guard = UPDATE_LOCK.lock().await;
    let args: Vec<String> = ["--update-to", "nightly", "--no-warnings"].map(String::from).to_vec();
    let mut cmd = command(&args);
    match timeout(Duration::from_secs(30), cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            let _ = fs::write(&marker, b"");
        }
        Ok(Ok(out)) => store::log_error(&format!(
            "yt-dlp update failed: {}",
            error_line(&String::from_utf8_lossy(&out.stderr))
        )),
        Ok(Err(e)) => store::log_error(&format!("yt-dlp update failed: {e}")),
        Err(_) => store::log_error("yt-dlp update timed out"),
    }
}

// ----------------------------------------------------------------- running

struct Output {
    stdout: String,
    stderr: String,
}

async fn run(args: &[String], secs: u64) -> Result<Output, String> {
    drop(UPDATE_LOCK.lock().await); // wait for a running self-update, then go
    let mut cmd = command(args);
    let out = timeout(Duration::from_secs(secs), cmd.output())
        .await
        .map_err(|_| "yt-dlp timed out".to_string())?
        .map_err(|e| format!("could not start yt-dlp: {e}"))?;
    Ok(Output {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

fn is_bot_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    s.contains("confirm you") && s.contains("bot")
}

fn error_line(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .find(|l| l.contains("ERROR"))
        .or_else(|| stderr.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn cookie_file() -> PathBuf {
    store::data_dir().join("cookies.txt")
}

#[tauri::command]
pub fn cookies_status() -> bool {
    cookie_file().exists()
}

/// Accepts only a YouTube/Google Netscape-format cookies file and makes sure it has the header
/// line yt-dlp insists on.
fn normalize_cookies(text: &str) -> Result<String, String> {
    let text = text.trim_start_matches('\u{feff}');
    let looks_ok = text.lines().any(|l| {
        l.split('\t').count() >= 7 && (l.contains("youtube.com") || l.contains("google.com"))
    });
    if !looks_ok {
        return Err("That doesn't look like a YouTube cookies.txt (Netscape format).".into());
    }
    let has_header = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .map_or(false, |l| l.starts_with("# Netscape") || l.starts_with("# HTTP"));
    Ok(if has_header {
        text.to_string()
    } else {
        format!("# Netscape HTTP Cookie File\n{text}")
    })
}

/// Stores a cookies.txt exported from a browser where you are signed in to YouTube.
#[tauri::command]
pub fn save_cookies(text: String) -> Result<(), String> {
    let body = normalize_cookies(&text)?;
    fs::write(cookie_file(), body).map_err(|e| e.to_string())?;
    let mut memory = MEMORY.lock().unwrap();
    memory.good = None;
    memory.browsers_failed_at = None;
    Ok(())
}

/// Startup check for a cookies.txt the installer copied in: tidy it up, or set it aside if invalid.
pub fn prepare_cookies() {
    let path = cookie_file();
    let Ok(raw) = fs::read_to_string(&path) else {
        return;
    };
    match normalize_cookies(&raw) {
        Ok(clean) if clean != raw => {
            let _ = fs::write(&path, clean);
        }
        Ok(_) => {}
        Err(_) => {
            let _ = fs::rename(&path, path.with_extension("txt.invalid"));
        }
    }
}

#[tauri::command]
pub fn clear_cookies() -> Result<(), String> {
    match fs::remove_file(cookie_file()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// The imported cookies.txt (or one placed next to the app).
fn find_cookie_file() -> Option<PathBuf> {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("cookies.txt")));
    [Some(cookie_file()), beside_exe]
        .into_iter()
        .flatten()
        .find(|p| p.exists())
}

fn plan() -> Vec<Strategy> {
    let mut order = vec![Strategy::Default];
    order.extend(CLIENT_FALLBACKS.into_iter().map(Strategy::Client));
    if find_cookie_file().is_some() {
        order.push(Strategy::CookieFile);
    }
    if !COOKIES_BROWSER.is_empty() {
        order.push(Strategy::Browser(COOKIES_BROWSER));
    } else {
        let cooling = MEMORY
            .lock()
            .unwrap()
            .browsers_failed_at
            .map_or(false, |t| t.elapsed() < AUTO_COOKIE_COOLDOWN);
        if !cooling {
            order.extend(AUTO_COOKIE_BROWSERS.into_iter().map(Strategy::Browser));
        }
    }
    order
}

fn strategy_args(strategy: Strategy) -> Option<Vec<String>> {
    Some(match strategy {
        Strategy::Default => Vec::new(),
        Strategy::Client(c) => vec![
            "--extractor-args".into(),
            format!("youtube:player_client={c}"),
        ],
        Strategy::CookieFile => vec![
            "--cookies".into(),
            find_cookie_file()?.to_string_lossy().into_owned(),
        ],
        Strategy::Browser(b) => vec!["--cookies-from-browser".into(), b.into()],
    })
}

/// Runs yt-dlp with each strategy in turn (the one that worked last time goes first) and
/// returns the first result that has output. A plain failure that is not the bot check
/// (private or removed video) is final straight away.
async fn run_smart(head: &[&str], url: &str, secs: u64) -> Result<Output, String> {
    let build = |extra: &[String]| -> Vec<String> {
        head.iter()
            .map(|s| s.to_string())
            .chain(extra.iter().cloned())
            .chain([url.to_string()])
            .collect()
    };

    let mut order = plan();
    let remembered = MEMORY.lock().unwrap().good;
    if let Some(i) = remembered.and_then(|g| order.iter().position(|s| *s == g)) {
        let first = order.remove(i);
        order.insert(0, first);
    }

    let mut first_failure: Option<Output> = None;
    let mut tried_browser = false;
    for strategy in order {
        let Some(extra) = strategy_args(strategy) else {
            continue;
        };
        let out = match run(&build(&extra), secs).await {
            Ok(out) => out,
            Err(e) if first_failure.is_none() => return Err(e),
            Err(_) => continue,
        };
        let has_output = !out.stdout.trim().is_empty();
        let plain_failure = strategy == Strategy::Default && !is_bot_error(&out.stderr);
        if has_output || plain_failure {
            if has_output {
                MEMORY.lock().unwrap().good = Some(strategy);
            }
            return Ok(out);
        }
        if remembered == Some(strategy) {
            MEMORY.lock().unwrap().good = None;
        }
        tried_browser |= matches!(strategy, Strategy::Browser(_));
        first_failure.get_or_insert(out);
    }

    if tried_browser {
        MEMORY.lock().unwrap().browsers_failed_at = Some(Instant::now());
    }
    Ok(first_failure.unwrap_or(Output {
        stdout: String::new(),
        stderr: String::new(),
    }))
}

/// Quick connectivity probe: three parallel connects, returns on the first success.
async fn is_online() -> bool {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<bool>(3);
    for addr in ["1.1.1.1:443", "8.8.8.8:53", "www.youtube.com:443"] {
        let tx = tx.clone();
        tokio::spawn(async move {
            let ok = timeout(Duration::from_millis(1200), TcpStream::connect(addr))
                .await
                .map_or(false, |r| r.is_ok());
            let _ = tx.send(ok).await;
        });
    }
    drop(tx);
    timeout(Duration::from_millis(1500), async {
        while let Some(ok) = rx.recv().await {
            if ok {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

// ----------------------------------------------------------------- playlists

fn fmt_time(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn parse_tracks(stdout: &str) -> Vec<Track> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '|');
            let (id, dur, title) = (parts.next()?, parts.next()?, parts.next()?);
            let title = title.trim();
            if id.is_empty() || title == "[Private video]" || title == "[Deleted video]" {
                return None;
            }
            let secs = dur.trim().parse::<f64>().map_or(0, |d| d as u64);
            Some(Track {
                id: id.to_string(),
                title: if title.is_empty() { "Unknown Track" } else { title }.to_string(),
                dur: if secs > 0 { fmt_time(secs) } else { "--:--".into() },
                secs,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn fetch_playlist(req: u64, url: String) -> PlaylistResult {
    PLAYLIST_REQ.fetch_max(req, Ordering::SeqCst);
    let empty = |error: String| PlaylistResult {
        tracks: Vec::new(),
        error,
    };
    if req != PLAYLIST_REQ.load(Ordering::SeqCst) {
        return empty(String::new()); // the user already clicked something else
    }

    let head = [
        "--flat-playlist",
        "--ignore-errors",
        "--no-warnings",
        "--no-cache-dir",
        "--socket-timeout",
        "15",
        "--print",
        "%(id)s|%(duration)s|%(title)s",
    ];
    match run_smart(&head, &url, 180).await {
        Ok(out) => {
            let tracks = parse_tracks(&out.stdout);
            if tracks.is_empty() {
                let error = error_line(&out.stderr);
                if !error.is_empty() {
                    store::log_error(&error);
                }
                return empty(error);
            }
            PlaylistResult {
                tracks,
                error: String::new(),
            }
        }
        Err(e) => {
            store::log_error(&e);
            empty(e)
        }
    }
}

// ----------------------------------------------------------------- streams

#[tauri::command]
pub async fn fetch_stream(req: u64, video_id: String) -> StreamResult {
    STREAM_REQ.fetch_max(req, Ordering::SeqCst);
    let mut url = String::new();
    let mut error = String::new();
    let mut auth_blocked = false;

    {
        let _one_at_a_time = STREAM_LOCK.lock().await;
        if req == STREAM_REQ.load(Ordering::SeqCst) {
            let head = [
                "-f",
                "bestaudio/best",
                "--no-playlist",
                "--no-warnings",
                "--no-cache-dir",
                "--socket-timeout",
                "15",
                "-g",
            ];
            let page = format!("https://www.youtube.com/watch?v={video_id}");
            match run_smart(&head, &page, 90).await {
                Ok(out) => {
                    url = out.stdout.lines().next().unwrap_or("").trim().to_string();
                    if url.is_empty() {
                        auth_blocked = is_bot_error(&out.stderr);
                        error = error_line(&out.stderr);
                    }
                }
                Err(e) => error = e,
            }
        }
    }

    if !error.is_empty() {
        store::log_error(&error);
    }
    // tells "no internet" apart from "bad video"
    let offline = url.is_empty() && req == STREAM_REQ.load(Ordering::SeqCst) && !is_online().await;
    StreamResult {
        url,
        offline,
        auth_blocked,
        error,
    }
}
