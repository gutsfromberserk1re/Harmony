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
const AUTO_COOKIE_COOLDOWN: Duration = Duration::from_secs(600);
const UPDATE_INTERVAL: Duration = Duration::from_secs(12 * 3600);

static PLAYLIST_REQ: AtomicU64 = AtomicU64::new(0);
static STREAM_REQ: AtomicU64 = AtomicU64::new(0);
/// One stream lookup at a time.
static STREAM_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
/// Held while yt-dlp updates itself so no lookup starts on a half-replaced exe.
static UPDATE_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
static EXE: OnceLock<PathBuf> = OnceLock::new();

struct CookieState {
    browser: Option<&'static str>,
    failed_at: Option<Instant>,
}
static COOKIES: Mutex<CookieState> = Mutex::new(CookieState {
    browser: None,
    failed_at: None,
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
    let args = vec!["-U".to_string(), "--no-warnings".to_string()];
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

/// cookies.txt next to the app (or in the data folder) wins, then the manual browser override.
fn explicit_cookie_args() -> Vec<String> {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("cookies.txt")));
    let candidates = [beside_exe, Some(store::data_dir().join("cookies.txt"))];
    if let Some(file) = candidates.into_iter().flatten().find(|p| p.exists()) {
        return vec!["--cookies".into(), file.to_string_lossy().into_owned()];
    }
    if !COOKIES_BROWSER.is_empty() {
        return vec!["--cookies-from-browser".into(), COOKIES_BROWSER.into()];
    }
    Vec::new()
}

/// Runs yt-dlp without cookies; if YouTube shows the bot check, retries with cookies borrowed
/// from an installed browser (and remembers the one that worked).
async fn run_smart(head: &[&str], url: &str, secs: u64) -> Result<Output, String> {
    let explicit = explicit_cookie_args();
    let build = |cookies: &[String]| -> Vec<String> {
        head.iter()
            .map(|s| s.to_string())
            .chain(cookies.iter().cloned())
            .chain([url.to_string()])
            .collect()
    };
    let browser_args = |b: &str| vec!["--cookies-from-browser".to_string(), b.to_string()];

    let remembered = COOKIES.lock().unwrap().browser;
    if let Some(b) = remembered {
        let out = run(&build(&browser_args(b)), secs).await?;
        if !is_bot_error(&out.stderr) {
            return Ok(out);
        }
        COOKIES.lock().unwrap().browser = None; // cookies expired / signed out: search again
    }

    let first = run(&build(&explicit), secs).await?;
    if !is_bot_error(&first.stderr) || !explicit.is_empty() {
        return Ok(first);
    }
    let failed_at = COOKIES.lock().unwrap().failed_at;
    if failed_at.map_or(false, |t| t.elapsed() < AUTO_COOKIE_COOLDOWN) {
        return Ok(first);
    }

    for b in AUTO_COOKIE_BROWSERS {
        if let Ok(out) = run(&build(&browser_args(b)), secs).await {
            if !out.stdout.trim().is_empty() && !is_bot_error(&out.stderr) {
                COOKIES.lock().unwrap().browser = Some(b);
                return Ok(out);
            }
        } // browser missing / locked / not signed in: try the next one
    }
    COOKIES.lock().unwrap().failed_at = Some(Instant::now());
    Ok(first)
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
