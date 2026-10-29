//! Everything that talks to the bundled yt-dlp.exe.
//!
//! The installer drops `yt-dlp.exe` next to the app. On first start it is copied to
//! `%LOCALAPPDATA%\Harmony` (always writable) and updated there with `yt-dlp -U`.

use serde::Serialize;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::{
    net::TcpStream,
    process::Command,
    sync::{Mutex as AsyncMutex, RwLock},
    time::timeout,
};

use crate::{media_proxy, store};

/// Optional manual override: "firefox", "chrome", "edge", ... ("" = automatic).
/// You can also place a cookies.txt (Netscape format) next to the app or in the data folder.
const COOKIES_BROWSER: &str = "";
/// If YouTube shows its bot check, these browsers are tried in order for a signed-in session.
const AUTO_COOKIE_BROWSERS: [&str; 4] = ["firefox", "chrome", "edge", "brave"];
/// YouTube app "clients" that often work without a sign-in. Which ones do changes over time.
const CLIENT_FALLBACKS: [&str; 3] = ["android_vr", "tv", "web_safari"];
const AUTO_COOKIE_COOLDOWN: Duration = Duration::from_secs(600);
const UPDATE_INTERVAL: Duration = Duration::from_secs(12 * 3600);
/// After a failed self-update, wait this long before trying again (instead of on every launch).
const UPDATE_RETRY: Duration = Duration::from_secs(3600);

/// Time budget per yt-dlp attempt / for all attempts of one lookup together.
const STREAM_ATTEMPT_SECS: u64 = 45;
const STREAM_TOTAL_SECS: u64 = 150;
const PLAYLIST_ATTEMPT_SECS: u64 = 120;
const PLAYLIST_TOTAL_SECS: u64 = 300;

/// Only direct HTTP(S) audio can be played by the <audio> element. HLS (m3u8) / DASH manifests
/// make yt-dlp print a URL that then never plays, so they are not allowed here: a strategy that
/// can only offer those counts as failed and the next strategy gets its turn.
const STREAM_FORMAT: &str = "ba[protocol=https]/ba[protocol=http]/b[protocol=https]/b[protocol=http]";

static PLAYLIST_REQ: AtomicU64 = AtomicU64::new(0);
static STREAM_REQ: AtomicU64 = AtomicU64::new(0);
/// One stream lookup at a time.
static STREAM_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());
/// Every yt-dlp process holds this shared while it runs. The updater only takes it exclusively
/// for the instant it swaps the freshly updated exe in, so lookups never wait for a download.
static EXE_LOCK: RwLock<()> = RwLock::const_new(());
static EXE: OnceLock<PathBuf> = OnceLock::new();
/// Serialises writing cookies back to disk; bumped when the user imports/removes cookies so a
/// slow, older yt-dlp run can never overwrite a fresh import.
static COOKIE_COMMIT: Mutex<()> = Mutex::new(());
static COOKIE_GEN: AtomicU64 = AtomicU64::new(0);
static COOKIE_RUN: AtomicU64 = AtomicU64::new(0);
/// The strategy that produced the URL we last handed out (and for which video).
static LAST_STREAM: OnceLock<Mutex<HashMap<String, Strategy>>> = OnceLock::new();

fn last_stream() -> &'static Mutex<HashMap<String, Strategy>> {
    LAST_STREAM.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Error text of a lookup that was abandoned because a newer one (another track) replaced it.
const SUPERSEDED: &str = "superseded";

/// Ways of reaching YouTube, tried in this order until one returns something:
/// plain, alternate player clients (no sign-in), the imported cookies.txt, browser cookies.
#[derive(Clone, Copy, PartialEq, Debug)]
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

fn command_for(program: &Path, args: &[String]) -> Command {
    let mut cmd = Command::new(program);
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

fn command(args: &[String]) -> Command {
    command_for(exe(), args)
}

fn cache_dir() -> PathBuf {
    let dir = store::data_dir().join("ytdlp-cache");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn age_of(path: &Path) -> Option<Duration> {
    modified(path)?.elapsed().ok()
}

/// Called once at startup. Updates a *copy* of yt-dlp in the background (at most every 12 hours)
/// and swaps it in only when it runs and no lookup is using the exe. Lookups never wait for it.
pub async fn prepare_and_update() {
    let path = exe().clone();
    let data = store::data_dir();
    let (ok_marker, fail_marker) = (data.join("ytdlp.checked"), data.join("ytdlp.failed"));
    if !path.starts_with(&data)
        || age_of(&ok_marker).map_or(false, |a| a < UPDATE_INTERVAL)
        || age_of(&fail_marker).map_or(false, |a| a < UPDATE_RETRY)
    {
        return;
    }
    tokio::time::sleep(Duration::from_secs(15)).await; // let the first lookups go first

    let staged = data.join("yt-dlp.update.exe");
    let cleanup = || {
        let _ = fs::remove_file(&staged);
        let _ = fs::remove_file(data.join("yt-dlp.update.exe.old"));
    };
    cleanup();
    let fail = |why: String| {
        store::log_error(&format!("yt-dlp update failed: {why}"));
        let _ = fs::write(&fail_marker, b"");
    };
    if let Err(e) = fs::copy(&path, &staged) {
        return fail(e.to_string());
    }

    let update: Vec<String> = ["--update-to", "nightly", "--no-warnings"].map(String::from).to_vec();
    match timeout(Duration::from_secs(180), command_for(&staged, &update).output()).await {
        Ok(Ok(out)) if out.status.success() => {}
        Ok(Ok(out)) => {
            cleanup();
            return fail(error_line(&String::from_utf8_lossy(&out.stderr)));
        }
        Ok(Err(e)) => {
            cleanup();
            return fail(e.to_string());
        }
        Err(_) => {
            cleanup();
            return fail("timed out".into());
        }
    }
    // the updated copy must still start before it replaces the working one
    let version: Vec<String> = vec!["--version".into()];
    let sane = matches!(
        timeout(Duration::from_secs(20), command_for(&staged, &version).output()).await,
        Ok(Ok(o)) if o.status.success()
    );
    if !sane {
        cleanup();
        return fail("updated yt-dlp does not start".into());
    }
    for _ in 0..150 {
        if let Ok(_exclusive) = EXE_LOCK.try_write() {
            match fs::rename(&staged, &path) {
                Ok(()) => {
                    let _ = fs::write(&ok_marker, b"");
                    let _ = fs::remove_file(&fail_marker);
                }
                Err(e) => fail(e.to_string()),
            }
            cleanup();
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    cleanup();
    fail("exe stayed busy".into());
}

// ----------------------------------------------------------------- running

struct Output {
    stdout: String,
    stderr: String,
}

/// Completes as soon as a stream lookup newer than `req` has been asked for.
async fn superseded(req: u64) {
    while STREAM_REQ.load(Ordering::SeqCst) <= req {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Runs yt-dlp. With `req` set, the process is killed the moment a newer stream request arrives
/// (the user skipped to another track), so quick next/previous presses never queue up behind it.
async fn run(args: &[String], secs: u64, req: Option<u64>) -> Result<Output, String> {
    let _shared = EXE_LOCK.read().await; // only blocked for the instant an update is swapped in
    if req.map_or(false, |r| STREAM_REQ.load(Ordering::SeqCst) > r) {
        return Err(SUPERSEDED.to_string());
    }
    let mut cmd = command(args);
    let work = timeout(Duration::from_secs(secs), cmd.output());
    let finished = match req {
        // dropping `work` kills the child process (kill_on_drop)
        Some(r) => tokio::select! {
            res = work => res,
            _ = superseded(r) => return Err(SUPERSEDED.to_string()),
        },
        None => work.await,
    };
    let out = finished
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

/// Errors where no other way of reaching YouTube can help.
fn is_permanent(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    [
        "has been removed",
        "has been deleted",
        "no longer available",
        "has been terminated",
        "available in your country",
        "blocked it in your country",
        "playlist does not exist",
    ]
    .iter()
    .any(|m| s.contains(m))
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

// ----------------------------------------------------------------- cookies

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

/// A cookies file without a signed-in session cannot help YouTube at all (this is the usual reason
/// "importing cookies changes nothing"): exported from a window that was not signed in, or from
/// the wrong site.
fn has_session(text: &str) -> bool {
    const NAMES: [&str; 10] = [
        "SID", "SAPISID", "APISID", "HSID", "SSID", "LOGIN_INFO",
        "__Secure-1PSID", "__Secure-3PSID", "__Secure-1PAPISID", "__Secure-3PAPISID",
    ];
    text.lines().any(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        f.len() >= 7
            && (f[0].contains("youtube.com") || f[0].contains("google.com"))
            && NAMES.contains(&f[5].trim())
    })
}

/// Stores a cookies.txt exported from a browser where you are signed in to YouTube.
#[tauri::command]
pub fn save_cookies(text: String) -> Result<(), String> {
    let body = normalize_cookies(&text)?;
    if !has_session(&body) {
        return Err("No signed-in YouTube session in this file. Sign in first, then export again.".into());
    }
    {
        let _commit = COOKIE_COMMIT.lock().unwrap();
        COOKIE_GEN.fetch_add(1, Ordering::SeqCst);
        write_atomic(&cookie_file(), &body)?;
    }
    let mut memory = MEMORY.lock().unwrap();
    memory.good = None;
    memory.browsers_failed_at = None;
    Ok(())
}

fn write_atomic(target: &Path, text: &str) -> Result<(), String> {
    let tmp = target.with_extension("txt.tmp");
    fs::write(&tmp, text).map_err(|e| e.to_string())?;
    fs::rename(&tmp, target).map_err(|e| e.to_string())
}

/// Startup check for a cookies.txt the installer copied in: tidy it up, or set it aside if invalid.
pub fn prepare_cookies() {
    // leftovers of runs that were killed
    if let Ok(dir) = fs::read_dir(store::data_dir()) {
        for e in dir.flatten() {
            if e.file_name().to_string_lossy().starts_with("cookies.run") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
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
    let _commit = COOKIE_COMMIT.lock().unwrap();
    COOKIE_GEN.fetch_add(1, Ordering::SeqCst);
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

/// yt-dlp rewrites the cookies file it is given when it exits. Several lookups run at the same
/// time (playlist, track, prefetch), so each one works on its own private copy; handing them the
/// real file made them overwrite each other and left a truncated cookies.txt behind.
struct StagedCookies {
    path: PathBuf,
    original: String,
    generation: u64,
}

fn stage_cookies() -> Option<StagedCookies> {
    let src = find_cookie_file()?;
    let original = fs::read_to_string(&src).ok()?;
    let generation = COOKIE_GEN.load(Ordering::SeqCst);
    let path = store::data_dir().join(format!("cookies.run{}.txt", COOKIE_RUN.fetch_add(1, Ordering::SeqCst)));
    fs::write(&path, &original).ok()?;
    Some(StagedCookies { path, original, generation })
}

/// Keeps the cookies YouTube refreshed during the run, but only if the copy is still a valid
/// cookies file and the user has not imported/removed cookies in the meantime.
fn finish_cookies(staged: StagedCookies) {
    if let Ok(now) = fs::read_to_string(&staged.path) {
        if now != staged.original {
            if let Ok(clean) = normalize_cookies(&now) {
                let _commit = COOKIE_COMMIT.lock().unwrap();
                if COOKIE_GEN.load(Ordering::SeqCst) == staged.generation {
                    let _ = write_atomic(&cookie_file(), &clean);
                }
            }
        }
    }
    let _ = fs::remove_file(&staged.path);
}

// ----------------------------------------------------------------- strategies

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

fn strategy_args(strategy: Strategy, cookies: Option<&StagedCookies>) -> Vec<String> {
    match strategy {
        Strategy::Default => Vec::new(),
        Strategy::Client(c) => vec![
            "--extractor-args".into(),
            format!("youtube:player_client={c}"),
        ],
        Strategy::CookieFile => cookies
            .map(|c| vec!["--cookies".into(), c.path.to_string_lossy().into_owned()])
            .unwrap_or_default(),
        Strategy::Browser(b) => vec!["--cookies-from-browser".into(), b.into()],
    }
}

/// Runs yt-dlp with each strategy in turn (the one that worked last time goes first, the one that
/// gave an unplayable URL last time goes last) and returns the first result that has output,
/// together with the strategy that produced it. Only errors that no strategy can fix (removed or
/// region-blocked video) end the search early; anything else - sign-in, 403, "format not
/// available", a timeout - moves on to the next way of reaching YouTube.
async fn run_smart(
    head: &[&str],
    url: &str,
    attempt_secs: u64,
    total_secs: u64,
    req: Option<u64>,
    avoid: Option<Strategy>,
) -> Result<(Output, Option<Strategy>), String> {
    let cache = cache_dir().to_string_lossy().into_owned();
    let build = |extra: &[String]| -> Vec<String> {
        ["--cache-dir".to_string(), cache.clone()]
            .into_iter()
            .chain(head.iter().map(|s| s.to_string()))
            .chain(extra.iter().cloned())
            .chain([url.to_string()])
            .collect()
    };

    let mut order = plan();
    let remembered = MEMORY.lock().unwrap().good;
    if let Some(a) = avoid {
        let mut memory = MEMORY.lock().unwrap();
        if memory.good == Some(a) {
            memory.good = None; // it hands out URLs that do not play: stop preferring it
        }
    }
    if let Some(i) = remembered
        .filter(|g| Some(*g) != avoid)
        .and_then(|g| order.iter().position(|s| *s == g))
    {
        let first = order.remove(i);
        order.insert(0, first);
    }
    if let Some(i) = avoid.and_then(|a| order.iter().position(|s| *s == a)) {
        let last = order.remove(i);
        order.push(last);
    }

    let deadline = Instant::now() + Duration::from_secs(total_secs);
    let mut failure: Option<Output> = None;
    let mut tried_browser = false;
    for strategy in order {
        let left = deadline.saturating_duration_since(Instant::now()).as_secs();
        if left < 5 {
            break;
        }
        let staged = if strategy == Strategy::CookieFile { stage_cookies() } else { None };
        if strategy == Strategy::CookieFile && staged.is_none() {
            continue;
        }
        let extra = strategy_args(strategy, staged.as_ref());
        let result = run(&build(&extra), attempt_secs.min(left), req).await;
        if let Some(s) = staged {
            finish_cookies(s);
        }
        let out = match result {
            Ok(out) => out,
            Err(e) if e == SUPERSEDED || e.starts_with("could not start") => return Err(e),
            Err(e) => {
                store::log_error(&format!("{strategy:?}: {e}"));
                tried_browser |= matches!(strategy, Strategy::Browser(_));
                continue; // a timeout is not final: the next strategy may be quicker
            }
        };
        if !out.stdout.trim().is_empty() {
            MEMORY.lock().unwrap().good = Some(strategy);
            return Ok((out, Some(strategy)));
        }
        store::log_error(&format!("{strategy:?}: {}", error_line(&out.stderr)));
        if is_permanent(&out.stderr) {
            return Ok((out, None));
        }
        if remembered == Some(strategy) {
            MEMORY.lock().unwrap().good = None;
        }
        tried_browser |= matches!(strategy, Strategy::Browser(_));
        // keep the most telling failure (the bot check is what the UI reacts to)
        let better = failure
            .as_ref()
            .map_or(true, |f| !is_bot_error(&f.stderr) && is_bot_error(&out.stderr));
        if better {
            failure = Some(out);
        }
    }

    if tried_browser {
        MEMORY.lock().unwrap().browsers_failed_at = Some(Instant::now());
    }
    Ok((
        failure.unwrap_or(Output { stdout: String::new(), stderr: String::new() }),
        None,
    ))
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
        "--socket-timeout",
        "15",
        "--print",
        "%(id)s|%(duration)s|%(title)s",
    ];
    match run_smart(&head, &url, PLAYLIST_ATTEMPT_SECS, PLAYLIST_TOTAL_SECS, None, None).await {
        Ok((out, _)) => {
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

/// `-j` preserves the URL and the request headers associated with the selected format.
/// The frontend cannot attach those headers itself; the local media relay uses both.
fn parse_stream_metadata(stdout: &str) -> Result<(String, HashMap<String, String>), String> {
    let info: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("yt-dlp returned invalid JSON: {e}"))?;
    let url = info
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| "yt-dlp JSON did not include a direct media URL".to_string())?
        .to_string();
    let mut headers = HashMap::new();
    if let Some(object) = info.get("http_headers").and_then(serde_json::Value::as_object) {
        for (name, value) in object {
            if let Some(value) = value.as_str() {
                headers.insert(name.clone(), value.to_string());
            }
        }
    }
    Ok((url, headers))
}

/// `req` is the id of the newest *foreground* lookup. The background prefetch of the next track
/// passes the id that is already current (it does not count up), so it can never cancel a track
/// the user asked for, while the next foreground lookup cancels it. `retry` = the URL handed out
/// for this video last time did not play: try a different way of reaching YouTube.
#[tauri::command]
pub async fn fetch_stream(req: u64, video_id: String, retry: Option<bool>) -> StreamResult {
    STREAM_REQ.fetch_max(req, Ordering::SeqCst);
    let mut url = String::new();
    let mut error = String::new();
    let mut auth_blocked = false;

    let avoid = if retry.unwrap_or(false) {
        last_stream()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&video_id)
            .copied()
    } else {
        None
    };

    {
        let _one_at_a_time = STREAM_LOCK.lock().await;
        if req == STREAM_REQ.load(Ordering::SeqCst) {
            let head = [
                "-f",
                STREAM_FORMAT,
                "--no-playlist",
                "--no-warnings",
                "--socket-timeout",
                "15",
                "--dump-single-json",
            ];
            let page = format!("https://www.youtube.com/watch?v={video_id}");
            match run_smart(&head, &page, STREAM_ATTEMPT_SECS, STREAM_TOTAL_SECS, Some(req), avoid).await {
                Ok((out, used)) => {
                    match parse_stream_metadata(&out.stdout) {
                        Ok((upstream_url, headers)) => match media_proxy::register(upstream_url, headers) {
                            Ok(local_url) => {
                                url = local_url;
                                if let Some(strategy) = used {
                                    let mut last = last_stream().lock().unwrap_or_else(|p| p.into_inner());
                                    last.insert(video_id.clone(), strategy);
                                    if last.len() > 256 {
                                        if let Some(key) = last.keys().next().cloned() {
                                            last.remove(&key);
                                        }
                                    }
                                }
                            }
                            Err(e) => error = e,
                        },
                        Err(e) => {
                            auth_blocked = is_bot_error(&out.stderr);
                            error = format!("Could not read yt-dlp stream metadata: {e}");
                        }
                    }
                }
                Err(e) => error = e,
            }
        } else {
            error = SUPERSEDED.to_string(); // a newer track was requested while this one waited
        }
    }

    if !error.is_empty() && error != SUPERSEDED {
        store::log_error(&error);
    }
    // tells "no internet" apart from "bad video" (skipped for abandoned lookups)
    let offline = url.is_empty()
        && error != SUPERSEDED
        && req == STREAM_REQ.load(Ordering::SeqCst)
        && !is_online().await;
    StreamResult {
        url,
        offline,
        auth_blocked,
        error,
    }
}
