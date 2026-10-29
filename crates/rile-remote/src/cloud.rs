//! HTTP plumbing shared by the Google Drive and OneDrive backends: bearer
//! tokens, retries for throttling and expired tokens, error mapping.

use std::fmt;
use std::io::Read;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use ureq::http::{Request, Response};

use crate::oauth::{self, SharedTokens};
use crate::{ConnectionInfo, ConnectionLost};

/// The API answered with an error status.
#[derive(Debug, Clone)]
pub(crate) struct HttpError {
    pub status: u16,
    pub message: String,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (HTTP {})", self.message, self.status)
    }
}
impl std::error::Error for HttpError {}

pub(crate) fn status_of(err: &anyhow::Error) -> Option<u16> {
    err.chain().find_map(|e| e.downcast_ref::<HttpError>()).map(|e| e.status)
}

pub(crate) struct Api {
    agent: ureq::Agent,
    tokens: SharedTokens,
}

/// A request body: bytes plus their content type.
pub(crate) type Payload<'a> = Option<(&'a [u8], &'a str)>;

impl Api {
    pub(crate) fn new(info: &ConnectionInfo, secret: Option<&str>, follow_redirects: bool) -> Result<Self> {
        Ok(Self { agent: oauth::agent(follow_redirects), tokens: oauth::tokens_for(info, secret)? })
    }

    /// Send a request and return any non-error response (2xx, or 3xx when
    /// redirects aren't followed). `auth = false` for pre-authorized upload URLs.
    pub(crate) fn call(&self, method: &str, url: &str, headers: &[(&str, &str)], body: Payload, auth: bool) -> Result<Response<ureq::Body>> {
        let mut refreshed = false;
        let mut attempt = 0u32;
        loop {
            let mut req = Request::builder().method(method).uri(url);
            if auth {
                let token = oauth::access_token(&self.tokens, &self.agent, false)?;
                req = req.header("Authorization", format!("Bearer {token}"));
            }
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            let sent = match body {
                Some((bytes, content_type)) => self.agent.run(req.header("Content-Type", content_type).body(bytes)?),
                // POST/PATCH without a body still need a length for some servers.
                None if matches!(method, "POST" | "PATCH" | "PUT") => self.agent.run(req.body(&[][..])?),
                None => self.agent.run(req.body(())?),
            };
            let resp = match sent {
                Ok(resp) => resp,
                Err(e @ (ureq::Error::BadUri(_) | ureq::Error::Http(_) | ureq::Error::InvalidProxyUrl)) => {
                    return Err(anyhow::Error::new(e).context(format!("{method} {url}")));
                }
                Err(e) => return Err(ConnectionLost(e.to_string()).into()),
            };
            let status = resp.status().as_u16();
            if status == 401 && auth && !refreshed {
                refreshed = true;
                oauth::access_token(&self.tokens, &self.agent, true)?;
                continue;
            }
            if matches!(status, 429 | 500 | 502 | 503 | 504) && attempt < 4 {
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(1 << attempt)
                    .min(30);
                log::info!("{method} {url}: HTTP {status}, retrying in {wait}s");
                std::thread::sleep(Duration::from_secs(wait));
                attempt += 1;
                continue;
            }
            if status >= 400 {
                return Err(error_from(resp));
            }
            return Ok(resp);
        }
    }

    pub(crate) fn json<T: DeserializeOwned>(&self, method: &str, url: &str, body: Option<&serde_json::Value>) -> Result<T> {
        let bytes = body.map(|b| serde_json::to_vec(b).expect("json value"));
        let resp = self.call(method, url, &[], bytes.as_deref().map(|b| (b, "application/json; charset=UTF-8")), true)?;
        read_json(resp)
    }
}

pub(crate) fn read_json<T: DeserializeOwned>(resp: Response<ureq::Body>) -> Result<T> {
    let text = resp
        .into_body()
        .into_with_config()
        .limit(64 << 20)
        .read_to_string()
        .map_err(|e| ConnectionLost(e.to_string()))?;
    serde_json::from_str(&text).with_context(|| format!("unexpected response: {}", text.chars().take(200).collect::<String>()))
}

fn error_from(resp: Response<ureq::Body>) -> anyhow::Error {
    let status = resp.status().as_u16();
    let text = resp.into_body().into_with_config().limit(1 << 20).read_to_string().unwrap_or_default();
    // Google: {"error":{"message":..}}; Graph: {"error":{"code":..,"message":..}}.
    let message = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).map(str::to_string))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| match status {
            403 => "Access denied".into(),
            404 => "Not found".into(),
            _ => "Request failed".into(),
        });
    HttpError { status, message }.into()
}

/// Stream a response body to `sink` in chunks (`sink` returns false to stop).
pub(crate) fn stream(resp: Response<ureq::Body>, sink: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<()> {
    let mut reader = resp.into_body().into_reader();
    let mut buf = vec![0u8; crate::backend::CHUNK];
    loop {
        let n = reader.read(&mut buf).map_err(|e| ConnectionLost(e.to_string()))?;
        if n == 0 || !sink(&buf[..n])? {
            return Ok(());
        }
    }
}

/// Fill `buf` from `src` (short only at end of input). Errors from `src`
/// (e.g. cancellation) are passed through unchanged.
pub(crate) fn read_full(src: &mut dyn Read, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match src.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(filled)
}

/// Read exactly `size` bytes of upload input in chunks of `chunk` bytes and
/// call `send(offset, bytes)` for each one (once with no bytes for an empty file).
pub(crate) fn upload_chunks(src: &mut dyn Read, size: u64, chunk: usize, send: &mut dyn FnMut(u64, &[u8]) -> Result<()>) -> Result<()> {
    let mut buf = vec![0u8; chunk.min(size.max(1) as usize)];
    let mut offset = 0u64;
    loop {
        let want = buf.len().min((size - offset) as usize);
        let n = read_full(src, &mut buf[..want])?;
        if n < want {
            anyhow::bail!("the file changed while it was being uploaded");
        }
        send(offset, &buf[..n])?;
        offset += n as u64;
        if offset >= size {
            return Ok(());
        }
    }
}

pub(crate) fn parse_time(s: Option<&str>) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(s?).ok().map(SystemTime::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_exact_size() {
        let data: Vec<u8> = (0..25u8).collect();
        let mut seen = Vec::new();
        upload_chunks(&mut &data[..], 25, 10, &mut |off, b| {
            seen.push((off, b.len()));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, [(0, 10), (10, 10), (20, 5)]);
        let mut seen = Vec::new();
        upload_chunks(&mut &[][..], 0, 10, &mut |off, b| {
            seen.push((off, b.len()));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, [(0, 0)]);
        assert!(upload_chunks(&mut &data[..5], 25, 10, &mut |_, _| Ok(())).is_err());
    }

    #[test]
    fn parses_rfc3339() {
        let t = parse_time(Some("2024-05-01T12:00:00.000Z")).unwrap();
        assert_eq!(t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(), 1_714_564_800);
        assert!(parse_time(Some("nope")).is_none());
    }
}
