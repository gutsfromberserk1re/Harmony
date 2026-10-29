//! Local HTTP relay for YouTube media streams.
//!
//! yt-dlp's direct media URL is not always sufficient on its own: the URL may need the same
//! request headers yt-dlp used when resolving it. HTMLMediaElement cannot attach arbitrary
//! headers, so the WebView requests a loopback URL and this module forwards the request upstream.
//! Range requests are passed through to preserve seeking and incremental playback.

use futures_util::StreamExt;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_ENCODING, ACCEPT_RANGES, CONNECTION,
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, HOST, IF_RANGE, RANGE, REFERER, USER_AGENT,
};
use std::{
    collections::HashMap,
    net::TcpListener as StdTcpListener,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use crate::store;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

const MAX_REQUEST_HEADER_BYTES: usize = 32 * 1024;
const SOURCE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_SOURCES: usize = 512;

#[derive(Clone)]
struct StreamSource {
    url: String,
    headers: HashMap<String, String>,
    created: Instant,
}

static BASE_URL: OnceLock<String> = OnceLock::new();
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static SOURCES: OnceLock<Mutex<HashMap<String, StreamSource>>> = OnceLock::new();
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn sources() -> &'static Mutex<HashMap<String, StreamSource>> {
    SOURCES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start the loopback-only relay. The operating system chooses a free port.
pub fn start() -> Result<(), String> {
    if BASE_URL.get().is_some() {
        return Ok(());
    }

    let listener = StdTcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("could not bind local media relay: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("could not configure local media relay: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("could not read local media relay address: {e}"))?
        .port();

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("could not initialize media HTTP client: {e}"))?;
    HTTP_CLIENT
        .set(client)
        .map_err(|_| "media HTTP client was already initialized".to_string())?;
    BASE_URL
        .set(format!("http://127.0.0.1:{port}"))
        .map_err(|_| "media relay was already initialized".to_string())?;

    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("[Harmony] media relay failed to start: {e}");
                return;
            }
        };
        loop {
            match listener.accept().await {
                Ok((socket, _)) => {
                    tokio::spawn(async move {
                        if let Err(e) = handle_request(socket).await {
                            eprintln!("[Harmony] media relay request failed: {e}");
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[Harmony] media relay accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    });

    Ok(())
}

/// Register an extracted upstream URL and return the loopback URL that the audio element should use.
pub fn register(url: String, headers: HashMap<String, String>) -> Result<String, String> {
    let base = BASE_URL
        .get()
        .ok_or_else(|| "local media relay has not started".to_string())?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("yt-dlp returned a non-HTTP media URL".to_string());
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let token = format!("{nanos:x}-{count:x}");

    let mut map = sources().lock().unwrap_or_else(|p| p.into_inner());
    map.retain(|_, source| source.created.elapsed() < SOURCE_TTL);
    if map.len() >= MAX_SOURCES {
        if let Some(oldest) = map
            .iter()
            .min_by_key(|(_, source)| source.created)
            .map(|(token, _)| token.clone())
        {
            map.remove(&oldest);
        }
    }
    map.insert(
        token.clone(),
        StreamSource {
            url,
            headers,
            created: Instant::now(),
        },
    );

    Ok(format!("{base}/stream/{token}"))
}

fn build_upstream_headers(
    source_headers: &HashMap<String, String>,
    requested_range: Option<&str>,
    if_range: Option<&str>,
) -> HeaderMap {
    let mut upstream_headers = HeaderMap::new();
    for (name, value) in source_headers {
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        if name == HOST
            || name == RANGE
            || name == IF_RANGE
            || name == ACCEPT_ENCODING
            || name == CONTENT_LENGTH
            || name == CONNECTION
        {
            continue;
        }
        let Ok(value) = HeaderValue::from_str(value) else {
            continue;
        };
        upstream_headers.insert(name, value);
    }
    upstream_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    if !upstream_headers.contains_key(ACCEPT) {
        upstream_headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    }
    if !upstream_headers.contains_key(REFERER) {
        upstream_headers.insert(REFERER, HeaderValue::from_static("https://www.youtube.com/"));
    }
    if !upstream_headers.contains_key(USER_AGENT) {
        upstream_headers.insert(
            USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0.0.0 Safari/537.36"),
        );
    }
    if let Some(range) = requested_range.and_then(|v| HeaderValue::from_str(v).ok()) {
        upstream_headers.insert(RANGE, range);
    }
    if let Some(range) = if_range.and_then(|v| HeaderValue::from_str(v).ok()) {
        upstream_headers.insert(IF_RANGE, range);
    }
    upstream_headers
}

/// Probe a direct media URL with a tiny byte-range request before exposing it to WebView2.
/// This catches some expired or rejected URLs before the player enters its reconnect loop.
pub async fn validate(url: &str, headers: &HashMap<String, String>) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("yt-dlp returned a non-HTTP media URL".into());
    }
    let client = HTTP_CLIENT
        .get()
        .ok_or_else(|| "media HTTP client is not initialized".to_string())?;
    let request_headers = build_upstream_headers(headers, Some("bytes=0-0"), None);
    let response = tokio::time::timeout(
        Duration::from_secs(8),
        client.get(url).headers(request_headers).send(),
    )
    .await
    .map_err(|_| "timed out validating YouTube media URL".to_string())?
    .map_err(|e| format!("could not validate YouTube media URL: {e}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown host".to_string());
    // A short upstream response fragment often distinguishes an expired URL from an explicit
    // PO-token/signature rejection. Keep it bounded and never log the signed media URL itself.
    let body_excerpt = match tokio::time::timeout(Duration::from_secs(1), response.text()).await {
        Ok(Ok(body)) => body
            .chars()
            .filter(|c| !c.is_control() || *c == ' ')
            .take(220)
            .collect::<String>(),
        _ => String::new(),
    };
    if body_excerpt.is_empty() {
        store::log_error(&format!(
            "media URL validation returned HTTP {} from {}",
            status.as_u16(), host
        ));
    } else {
        store::log_error(&format!(
            "media URL validation returned HTTP {} from {}; upstream says: {}",
            status.as_u16(), host, body_excerpt
        ));
    }
    Err(format!(
        "YouTube rejected this media URL (HTTP {}). Trying another stream source...",
        status.as_u16()
    ))
}

async fn handle_request(mut socket: TcpStream) -> Result<(), String> {
    let raw = read_request_headers(&mut socket).await?;
    let text = String::from_utf8_lossy(&raw);
    let mut lines = text.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();

    if method != "GET" && method != "HEAD" {
        write_simple_response(&mut socket, 405, "Method Not Allowed", "Only GET and HEAD are supported").await?;
        return Ok(());
    }

    let path = path.split('?').next().unwrap_or_default();
    let token = match path.strip_prefix("/stream/") {
        Some(token) if !token.is_empty() && !token.contains('/') => token,
        _ => {
            write_simple_response(&mut socket, 404, "Not Found", "Media stream not found").await?;
            return Ok(());
        }
    };

    let source = {
        let map = sources().lock().unwrap_or_else(|p| p.into_inner());
        map.get(token)
            .filter(|source| source.created.elapsed() < SOURCE_TTL)
            .cloned()
    };
    let source = match source {
        Some(source) => source,
        None => {
            write_simple_response(&mut socket, 404, "Not Found", "Media stream expired; retry playback").await?;
            return Ok(());
        }
    };

    let mut requested_range: Option<String> = None;
    let mut if_range: Option<String> = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "range" => requested_range = Some(value.trim().to_string()),
                "if-range" => if_range = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }

    let client = HTTP_CLIENT
        .get()
        .ok_or_else(|| "media HTTP client is not initialized".to_string())?;
    let upstream_headers = build_upstream_headers(
        &source.headers,
        requested_range.as_deref(),
        if_range.as_deref(),
    );

    let response = match client
        .get(&source.url)
        .headers(upstream_headers)
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            write_simple_response(&mut socket, 502, "Bad Gateway", "Could not fetch media from YouTube").await?;
            return Err(format!("upstream request failed: {e}"));
        }
    };

    let status = response.status();
    if !status.is_success() {
        let host = reqwest::Url::parse(&source.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| "unknown host".to_string());
        store::log_error(&format!("media relay upstream returned HTTP {} from {} (the stream URL may have expired or been rejected; retry will re-extract it)", status.as_u16(), host));
    }
    let reason = status.canonical_reason().unwrap_or("Upstream Response");
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("audio/webm");
    let content_length = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok());
    let content_range = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok());
    let accept_ranges = response
        .headers()
        .get(ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("bytes");

    let mut header = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), reason);
    header.push_str(&format!("Content-Type: {content_type}\r\n"));
    if let Some(length) = content_length {
        header.push_str(&format!("Content-Length: {length}\r\n"));
    }
    if let Some(range) = content_range {
        header.push_str(&format!("Content-Range: {range}\r\n"));
    }
    header.push_str(&format!("Accept-Ranges: {accept_ranges}\r\n"));
    header.push_str("Access-Control-Allow-Origin: *\r\n");
    header.push_str("Access-Control-Expose-Headers: Content-Length, Content-Range, Accept-Ranges\r\n");
    header.push_str("Connection: close\r\n\r\n");
    socket
        .write_all(header.as_bytes())
        .await
        .map_err(|e| format!("could not write media response headers: {e}"))?;

    if method != "HEAD" {
        let mut body = response.bytes_stream();
        while let Some(next) = body.next().await {
            match next {
                Ok(chunk) => {
                    if socket.write_all(&chunk).await.is_err() {
                        break; // client skipped tracks or closed the player
                    }
                }
                Err(e) => return Err(format!("upstream media stream interrupted: {e}")),
            }
        }
    }
    Ok(())
}

async fn read_request_headers(socket: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    loop {
        if data.len() > MAX_REQUEST_HEADER_BYTES {
            write_simple_response(socket, 431, "Request Header Fields Too Large", "Request headers too large").await?;
            return Err("incoming request headers exceeded the size limit".to_string());
        }
        let count = socket
            .read(&mut chunk)
            .await
            .map_err(|e| format!("could not read media request: {e}"))?;
        if count == 0 {
            return Err("client closed the connection before sending a request".to_string());
        }
        data.extend_from_slice(&chunk[..count]);
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            data.truncate(end);
            return Ok(data);
        }
    }
}

async fn write_simple_response(
    socket: &mut TcpStream,
    status: u16,
    reason: &str,
    message: &str,
) -> Result<(), String> {
    let body = message.as_bytes();
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    socket
        .write_all(header.as_bytes())
        .await
        .map_err(|e| format!("could not write error response: {e}"))?;
    socket
        .write_all(body)
        .await
        .map_err(|e| format!("could not write error response body: {e}"))?;
    Ok(())
}
