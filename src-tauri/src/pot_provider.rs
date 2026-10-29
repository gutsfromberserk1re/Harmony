//! On-demand local Proof-of-Origin token provider for yt-dlp.
//!
//! The provider and its yt-dlp plugin are downloaded and bundled by fetch-ytdlp.ps1.
//! The Deno server binds only to loopback and is started while extraction is active. It is
//! shut down after three minutes without extraction activity to keep idle memory use low.

use crate::store;
use std::{
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
const READY_TIMEOUT: Duration = Duration::from_secs(8);
const READY_POLL: Duration = Duration::from_millis(150);

struct ProviderState {
    server_root: Option<PathBuf>,
    deno_exe: Option<PathBuf>,
    port: Option<u16>,
    child: Option<Child>,
    last_used: Option<Instant>,
    ready: bool,
}

impl ProviderState {
    fn new() -> Self {
        Self {
            server_root: None,
            deno_exe: None,
            port: None,
            child: None,
            last_used: None,
            ready: false,
        }
    }
}

static STATE: OnceLock<Mutex<ProviderState>> = OnceLock::new();

fn state() -> &'static Mutex<ProviderState> {
    STATE.get_or_init(|| Mutex::new(ProviderState::new()))
}

fn free_loopback_port() -> Result<u16, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| format!("could not find a loopback port for the PO Token provider: {e}"))?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|e| format!("could not read PO Token provider port: {e}"))
}

fn find_deno() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    [
        exe_dir.join("deno-x86_64-pc-windows-msvc.exe"),
        exe_dir.join("deno.exe"),
        exe_dir.join("deno"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .or_else(|| Some(PathBuf::from("deno.exe")))
}

/// Install the provider plugin beside Harmony's yt-dlp executable and remember the bundled server.
/// The app still opens if resources are absent; non-provider client fallbacks remain available.
pub fn prepare(resource_dir: &Path, plugin_dir: &Path) -> Result<(), String> {
    let plugin_source = resource_dir
        .join("yt-dlp-plugins")
        .join("bgutil-ytdlp-pot-provider.zip");
    if !plugin_source.is_file() {
        return Err(format!("bundled PO Token plugin is missing: {}", plugin_source.display()));
    }

    fs::create_dir_all(plugin_dir)
        .map_err(|e| format!("could not create yt-dlp plugin directory: {e}"))?;
    let plugin_target = plugin_dir.join("bgutil-ytdlp-pot-provider.zip");
    fs::copy(&plugin_source, &plugin_target)
        .map_err(|e| format!("could not install the PO Token plugin: {e}"))?;

    let server_root = resource_dir.join("bgutil-provider").join("server");
    if !server_root.join("src").join("main.ts").is_file()
        || !server_root.join("node_modules").is_dir()
    {
        return Err(format!(
            "bundled PO Token server is incomplete: {}",
            server_root.display()
        ));
    }
    let deno_exe = find_deno().ok_or_else(|| "the bundled Deno runtime was not found".to_string())?;
    let port = free_loopback_port()?;

    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    guard.server_root = Some(server_root);
    guard.deno_exe = Some(deno_exe);
    guard.port = Some(port);
    guard.ready = false;
    store::log_error("PO Token provider plugin installed; provider server will start on demand");
    Ok(())
}

fn tcp_ready(port: u16) -> bool {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
}

fn start_blocking() -> Result<(), String> {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());

    let child_status = match guard.child.as_mut() {
        Some(child) => Some(child.try_wait()),
        None => None,
    };
    match child_status {
        Some(Ok(None)) => {
            if let Some(port) = guard.port {
                if tcp_ready(port) {
                    guard.ready = true;
                    guard.last_used = Some(Instant::now());
                    return Ok(());
                }
            }
            // A child that is alive but not listening is unhealthy; stop it before retrying.
            if let Some(mut child) = guard.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            guard.ready = false;
        }
        Some(Ok(Some(status))) => {
            store::log_error(&format!("PO Token provider exited unexpectedly: {status}"));
            guard.child = None;
            guard.ready = false;
        }
        Some(Err(e)) => {
            store::log_error(&format!("could not inspect PO Token provider process: {e}"));
            guard.child = None;
            guard.ready = false;
        }
        None => {}
    }

    let server_root = guard
        .server_root
        .clone()
        .ok_or_else(|| "PO Token provider resources have not been prepared".to_string())?;
    let deno_exe = guard
        .deno_exe
        .clone()
        .ok_or_else(|| "Deno runtime path has not been prepared".to_string())?;
    let node_modules = server_root.join("node_modules");
    if !node_modules.is_dir() {
        return Err("PO Token provider node_modules folder is missing".into());
    }

    let log_path = store::data_dir().join("pot-provider.log");
    let mut last_error = String::from("provider did not become ready before the timeout");
    for _ in 0..3 {
        let port = match free_loopback_port() {
            Ok(port) => port,
            Err(e) => return Err(e),
        };
        guard.port = Some(port);
        let stdout = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| format!("could not open PO Token provider log: {e}"))?;
        let stderr = stdout
            .try_clone()
            .map_err(|e| format!("could not duplicate PO Token provider log handle: {e}"))?;

        let mut command = Command::new(&deno_exe);
        command
            .arg("run")
            .arg("--allow-env")
            .arg("--allow-net")
            .arg("--allow-ffi=.")
            .arg("--allow-read=.")
            .arg("../src/main.ts")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .current_dir(&node_modules)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .env("DENO_NO_UPDATE_CHECK", "1")
            .env("DENO_DIR", store::data_dir().join("deno-cache"));
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let child = command
            .spawn()
            .map_err(|e| format!("could not start the bundled PO Token provider: {e}"))?;
        guard.child = Some(child);
        guard.ready = false;
        let deadline = Instant::now() + READY_TIMEOUT;
        while Instant::now() < deadline {
            if tcp_ready(port) {
                guard.ready = true;
                guard.last_used = Some(Instant::now());
                store::log_error(&format!("PO Token provider ready on loopback port {port}"));
                return Ok(());
            }
            let exited = guard
                .child
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten());
            if let Some(status) = exited {
                last_error = format!("PO Token provider exited during startup: {status}");
                guard.child = None;
                break;
            }
            thread::sleep(READY_POLL);
        }
        if let Some(mut child) = guard.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        guard.ready = false;
        last_error = format!("{last_error}; see {}", log_path.display());
    }
    Err(last_error)
}

pub async fn ensure_started() -> Result<(), String> {
    tokio::task::spawn_blocking(start_blocking)
        .await
        .map_err(|e| format!("PO Token provider startup task failed: {e}"))?
}

/// Refresh the inactivity timer during long yt-dlp requests.
pub fn touch() {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    if guard.child.is_some() {
        guard.last_used = Some(Instant::now());
    }
}

/// Appended to yt-dlp's extractor args only while the provider server is ready.
pub fn extractor_args() -> Option<String> {
    let guard = state().lock().unwrap_or_else(|p| p.into_inner());
    if guard.ready && guard.child.is_some() {
        guard.port.map(|port| {
            format!("youtubepot-bgutilhttp:base_url=http://127.0.0.1:{port}")
        })
    } else {
        None
    }
}

/// Stop the sidecar after three minutes without yt-dlp extraction activity.
pub async fn monitor_idle() {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
        let idle = guard.last_used.map_or(false, |last| last.elapsed() >= IDLE_TIMEOUT);
        if idle {
            if let Some(mut child) = guard.child.take() {
                let _ = child.kill();
                let _ = child.wait();
                store::log_error("PO Token provider stopped after inactivity");
            }
            guard.ready = false;
            guard.last_used = None;
        }
    }
}

pub fn stop() {
    let mut guard = state().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(mut child) = guard.child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    guard.ready = false;
    guard.last_used = None;
}
