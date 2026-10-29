//! Messages between Rile devices. Each frame is a kind byte, a big-endian
//! `u32` length and the payload: JSON for messages, raw bytes for file data
//! (an empty data frame ends a file).

use std::io::{self, Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::RemoteEntry;

/// Bumped on incompatible protocol changes.
pub const PROTOCOL_VERSION: u32 = 1;
const KIND_JSON: u8 = 0;
const KIND_DATA: u8 = 1;
const MAX_JSON: usize = 16 << 20;
const MAX_DATA: usize = 4 << 20;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Laptop,
    #[default]
    Desktop,
    Server,
}

/// What a device tells others about itself.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct DeviceInfo {
    /// Filled in by the receiver from the TLS certificate; never trusted from the wire.
    pub id: String,
    pub name: String,
    /// "MacBook Pro (Mac15,9)"
    pub model: String,
    /// "macOS 26.1"
    pub os: String,
    pub kind: DeviceKind,
    /// Installed RAM in bytes; 0 when unknown (older versions).
    pub memory: u64,
    pub storage_used: u64,
    pub storage_total: u64,
    /// Names of the folders it shares (only sent to paired devices).
    pub shares: Vec<String>,
    pub version: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct WireEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// Unix milliseconds.
    pub modified: Option<u64>,
    pub permissions: Option<u32>,
}

impl From<WireEntry> for RemoteEntry {
    fn from(e: WireEntry) -> Self {
        RemoteEntry {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            is_symlink: e.is_symlink,
            size: e.size,
            modified: e.modified.map(|ms| UNIX_EPOCH + Duration::from_millis(ms)),
            permissions: e.permissions,
        }
    }
}

pub(crate) fn millis(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

/// One item of a file offer. `path` is relative, `/`-separated, and starts
/// with the name of the top-level item it belongs to.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct OfferItem {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Request {
    Hello { version: u32, info: DeviceInfo },
    Pair,
    /// The requesting side's user confirmed (or refused) the code.
    PairConfirm { ok: bool },
    List { path: String },
    Stat { path: String },
    Read { path: String },
    /// Followed, once accepted, by the data of every file item in order.
    Offer { items: Vec<OfferItem>, total: u64 },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NotPaired,
    NotFound,
    Denied,
    Declined,
    Other,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "r", rename_all = "snake_case")]
pub(crate) enum Response {
    Hello { version: u32, info: DeviceInfo, trusted: bool },
    Paired { ok: bool },
    Entries { entries: Vec<WireEntry> },
    Entry { entry: WireEntry },
    /// `Read`: data frames follow. `Offer`: the receiver accepted; send data.
    Ok,
    /// `Offer`: all files written.
    Done,
    Error { kind: ErrorKind, message: String },
}

/// An error answer from the other device.
#[derive(Debug, Clone)]
pub struct PeerError {
    pub kind: ErrorKind,
    pub message: String,
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PeerError {}

/// Whether `err` is the other device saying it isn't paired with this one.
pub fn is_not_paired(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.downcast_ref::<PeerError>().is_some_and(|p| p.kind == ErrorKind::NotPaired))
}

pub(crate) enum Frame {
    Json(Vec<u8>),
    Data(Vec<u8>),
}

pub(crate) fn write_json(w: &mut impl Write, msg: &impl Serialize) -> io::Result<()> {
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    write_frame(w, KIND_JSON, &body)?;
    w.flush()
}

pub(crate) fn write_data(w: &mut impl Write, data: &[u8]) -> io::Result<()> {
    write_frame(w, KIND_DATA, data)
}

fn write_frame(w: &mut impl Write, kind: u8, body: &[u8]) -> io::Result<()> {
    // One write, so a frame is never split across TLS records needlessly.
    let mut frame = Vec::with_capacity(5 + body.len());
    frame.push(kind);
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);
    w.write_all(&frame)
}

pub(crate) fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_be_bytes(head[1..].try_into().unwrap_or_default()) as usize;
    let max = if head[0] == KIND_JSON { MAX_JSON } else { MAX_DATA };
    if len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    match head[0] {
        KIND_JSON => Ok(Frame::Json(body)),
        KIND_DATA => Ok(Frame::Data(body)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "unknown frame")),
    }
}

pub(crate) fn read_json<T: DeserializeOwned>(r: &mut impl Read) -> Result<T> {
    match read_frame(r)? {
        Frame::Json(body) => Ok(serde_json::from_slice(&body)?),
        Frame::Data(_) => bail!("unexpected data frame"),
    }
}

/// Next data chunk; `None` at the end of a file.
pub(crate) fn read_data(r: &mut impl Read) -> Result<Option<Vec<u8>>> {
    match read_frame(r)? {
        Frame::Data(d) if d.is_empty() => Ok(None),
        Frame::Data(d) => Ok(Some(d)),
        Frame::Json(body) => match serde_json::from_slice::<Response>(&body) {
            Ok(Response::Error { kind, message }) => Err(PeerError { kind, message }.into()),
            _ => Err(anyhow!("unexpected message in file data")),
        },
    }
}

/// Validates an offer path: relative, `/`-separated, only normal components.
pub(crate) fn safe_relative(path: &str) -> Option<std::path::PathBuf> {
    let mut out = std::path::PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') || part.contains(':') || part.contains('\0') {
            return None;
        }
        out.push(part);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_json(&mut buf, &Request::List { path: "/a".into() }).unwrap();
        write_data(&mut buf, b"hello").unwrap();
        write_data(&mut buf, b"").unwrap();
        let mut r = &buf[..];
        assert!(matches!(read_json::<Request>(&mut r).unwrap(), Request::List { path } if path == "/a"));
        assert_eq!(read_data(&mut r).unwrap().as_deref(), Some(&b"hello"[..]));
        assert_eq!(read_data(&mut r).unwrap(), None);
    }

    #[test]
    fn offer_paths_must_stay_relative() {
        assert!(safe_relative("Photos/a.jpg").is_some());
        for bad in ["", "/etc/passwd", "a/../../b", "..", "a//b", "C:/x", "a\\..\\b", "./a"] {
            assert!(safe_relative(bad).is_none(), "{bad}");
        }
    }
}
