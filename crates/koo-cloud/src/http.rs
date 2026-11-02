//! The HTTP client the providers share: one agent, a multipart upload that
//! streams its file and reports progress, and retries while a server is
//! busy or failing. What an error reply means is up to each provider.

use koo_core::cloud::CloudError;
use std::io::{Chain, Cursor, Read};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use ureq::http::Response;
use ureq::{Agent, Body, SendBody};

/// Waits after a busy or failed attempt, one per retry.
const BACKOFF: [u64; 4] = [2, 4, 8, 16];

/// Biggest reply read into memory: a long piece of music at a high bit rate.
const MAX_REPLY: u64 = 512 << 20;

fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        use ureq::tls::{RootCerts, TlsConfig};
        Agent::config_builder()
            .tls_config(
                TlsConfig::builder()
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .user_agent(concat!("koo-editor/", env!("CARGO_PKG_VERSION")))
            // Errors are read from the reply; a long file is heard for
            // minutes before the reply starts.
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60 * 60)))
            .timeout_recv_body(None)
            .build()
            .into()
    })
}

/// A reply with its body read.
pub struct Reply {
    pub status: u16,
    headers: ureq::http::HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    pub fn json(&self) -> Result<serde_json::Value, CloudError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| CloudError::Failed(format!("unexpected reply: {e}")))
    }

    /// Worth trying again: the server was busy, or failed when
    /// `server_errors` may be retried.
    fn retryable(&self, server_errors: bool) -> bool {
        self.status == 429 || (server_errors && self.status >= 500)
    }
}

fn read(mut response: Response<Body>) -> Result<Reply, ureq::Error> {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_REPLY)
        .read_to_vec()?;
    Ok(Reply {
        status,
        headers,
        body,
    })
}

fn network(e: ureq::Error) -> CloudError {
    CloudError::Failed(format!("could not reach the provider: {e}"))
}

/// Run `attempt` until it gets a reply that isn't busy (or a server error,
/// with `server_errors`), or the retries run out; the last reply is
/// returned either way. Requests that are charged for what they make leave
/// out `server_errors`: the work may have been done and billed.
fn with_retries(
    server_errors: bool,
    mut attempt: impl FnMut() -> Result<Response<Body>, ureq::Error>,
) -> Result<Reply, CloudError> {
    let mut waits = BACKOFF.iter();
    loop {
        let outcome = attempt().and_then(read);
        let wait = match &outcome {
            Ok(reply) if !reply.retryable(server_errors) => None,
            Err(_) if !server_errors => None,
            _ => waits.next(),
        };
        match (outcome, wait) {
            (outcome, Some(secs)) => {
                drop(outcome);
                std::thread::sleep(Duration::from_secs(*secs));
            }
            (Ok(reply), None) => return Ok(reply),
            (Err(e), None) => return Err(network(e)),
        }
    }
}

pub fn get(url: &str, headers: &[(&str, &str)]) -> Result<Reply, CloudError> {
    with_retries(true, || {
        let mut request = agent().get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.call()
    })
}

/// A JSON POST; see [`with_retries`] for `server_errors`.
pub fn post_json(
    url: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
    server_errors: bool,
) -> Result<Reply, CloudError> {
    let bytes = serde_json::to_vec(body).map_err(|e| CloudError::Failed(e.to_string()))?;
    with_retries(server_errors, || {
        let mut request = agent().post(url).header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send(&bytes[..])
    })
}

pub fn delete(url: &str, headers: &[(&str, &str)]) -> Result<Reply, CloudError> {
    with_retries(false, || {
        let mut request = agent().delete(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.call()
    })
}

/// A PUT with no body: asks a server to stop something.
pub fn put(url: &str, headers: &[(&str, &str)]) -> Result<Reply, CloudError> {
    with_retries(true, || {
        let mut request = agent().put(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send_empty()
    })
}

/// A PUT of the file at `path` as the whole body, read as it is sent: an
/// upload to a URL that was handed out for it, with the `headers` it was
/// handed out with (its media type, at least). Its length is added when
/// they don't carry it.
pub fn put_file(url: &str, path: &Path, headers: &[(&str, &str)]) -> Result<Reply, CloudError> {
    let size = std::fs::metadata(path)
        .map_err(|e| CloudError::Failed(format!("could not read {}: {e}", path.display())))?
        .len();
    let sized = headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
    with_retries(true, || {
        let file = std::fs::File::open(path)?;
        let mut request = agent().put(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if !sized {
            request = request.header("content-length", size.to_string());
        }
        request.send(SendBody::from_owned_reader(file))
    })
}

/// Fetch `url` into `path` (through a `.part` file), without holding it in
/// memory; `progress` gets 0..1 when the server says how long it is.
pub fn download(url: &str, path: &Path, progress: &dyn Fn(f32)) -> Result<(), CloudError> {
    let fail =
        |e: std::io::Error| CloudError::Failed(format!("could not write {}: {e}", path.display()));
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = std::path::PathBuf::from(part);
    let mut waits = BACKOFF.iter();
    let outcome = loop {
        let attempt = (|| -> Result<Result<(), CloudError>, ureq::Error> {
            let response = agent().get(url).call()?;
            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                return Ok(Err(CloudError::Failed(format!(
                    "the file could not be fetched ({status})"
                ))));
            }
            let total = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let mut reader = response.into_body().into_reader();
            let mut file = std::fs::File::create(&part)?;
            let mut buf = vec![0; 1 << 16];
            let mut got = 0u64;
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                std::io::Write::write_all(&mut file, &buf[..n])?;
                got += n as u64;
                if let Some(total) = total.filter(|t| *t > 0) {
                    progress((got as f32 / total as f32).min(1.0));
                }
            }
            file.sync_all()?;
            Ok(Ok(()))
        })();
        match (attempt, waits.next()) {
            (Ok(result), _) => break result,
            (Err(_), Some(secs)) => std::thread::sleep(Duration::from_secs(*secs)),
            (Err(e), None) => break Err(network(e)),
        }
    };
    match outcome {
        Ok(()) => std::fs::rename(&part, path).map_err(fail),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

/// A file in a [`post_form`]: its field, path and media type.
pub type FormFile<'a> = (&'a str, &'a Path, &'a str);

/// A `multipart/form-data` POST of `fields` and `files`, read into memory
/// first: for a few short recordings. See [`with_retries`] for
/// `server_errors`.
pub fn post_form(
    url: &str,
    headers: &[(&str, &str)],
    fields: &[(&str, String)],
    files: &[FormFile],
    server_errors: bool,
) -> Result<Reply, CloudError> {
    let boundary = boundary();
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    for (field, path, mime) in files {
        let bytes = std::fs::read(path)
            .map_err(|e| CloudError::Failed(format!("could not read {}: {e}", path.display())))?;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().replace('"', "'"))
            .unwrap_or_else(|| "audio".into());
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let content_type = format!("multipart/form-data; boundary={boundary}");
    with_retries(server_errors, || {
        let mut request = agent().post(url).header("content-type", &content_type);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send(&body[..])
    })
}

/// A multipart boundary no file is likely to hold.
fn boundary() -> String {
    format!(
        "koo-editor-{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

/// The media type of a picture, clip or sound, by its extension.
pub fn media_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("tif" | "tiff") => "image/tiff",
        Some("heic") => "image/heic",
        Some("heif") => "image/heif",
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        _ => audio_mime(path),
    }
}

/// The media type of an audio file, by its extension.
pub fn audio_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("m4a" | "mp4" | "aac") => "audio/mp4",
        Some("ogg" | "opus") => "audio/ogg",
        Some("webm") => "audio/webm",
        _ => "audio/mpeg",
    }
}

/// A `multipart/form-data` POST of `fields` and the file at `path` under
/// `field`, read as it is sent; `progress` gets 0..1 of it. See
/// [`with_retries`] for `server_errors`.
pub fn post_file(
    url: &str,
    headers: &[(&str, &str)],
    fields: &[(&str, String)],
    (field, path, mime): (&str, &Path, &str),
    progress: &(dyn Fn(f32) + Sync),
    server_errors: bool,
) -> Result<Reply, CloudError> {
    let boundary = boundary();
    let mut head = String::new();
    for (name, value) in fields {
        head.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".into());
    head.push_str(&format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{file_name}\"\r\nContent-Type: {mime}\r\n\r\n"
    ));
    let tail = format!("\r\n--{boundary}--\r\n");
    let size = std::fs::metadata(path)
        .map_err(|e| CloudError::Failed(format!("could not read {}: {e}", path.display())))?
        .len();
    let total = head.len() as u64 + size + tail.len() as u64;
    let content_type = format!("multipart/form-data; boundary={boundary}");
    with_retries(server_errors, || {
        let file = std::fs::File::open(path)?;
        let mut body = Counted {
            inner: Cursor::new(head.clone().into_bytes())
                .chain(file)
                .chain(Cursor::new(tail.clone().into_bytes())),
            sent: 0,
            total,
            progress,
        };
        let mut request = agent()
            .post(url)
            .header("content-type", &content_type)
            .header("content-length", total.to_string());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send(SendBody::from_reader(&mut body))
    })
}

type Parts = Chain<Chain<Cursor<Vec<u8>>, std::fs::File>, Cursor<Vec<u8>>>;

/// The body of an upload, counting what has been read of it.
struct Counted<'a> {
    inner: Parts,
    sent: u64,
    total: u64,
    progress: &'a (dyn Fn(f32) + Sync),
}

impl Read for Counted<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.sent += n as u64;
        (self.progress)(self.sent as f32 / self.total.max(1) as f32);
        Ok(n)
    }
}
