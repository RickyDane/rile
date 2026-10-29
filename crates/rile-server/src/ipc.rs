//! `rilectl` ↔ `rile-server` over a Unix socket: one JSON object per line.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Status,
    /// Re-read the configuration file.
    Reload,
    Forget { id: String },
    /// Keep the connection open and pass pair requests to it for `secs`.
    Pair { secs: u64 },
    /// The answer to a [`Response::PairRequest`].
    Answer { accept: bool },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub os: String,
    /// `host:port` when it is on the network right now.
    #[serde(default)]
    pub address: Option<String>,
    pub paired: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ShareStatus {
    pub name: String,
    pub path: PathBuf,
    pub writable: bool,
    /// Its folder exists (otherwise it isn't published).
    pub available: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Status {
    pub id: String,
    pub name: String,
    pub model: String,
    pub os: String,
    pub port: u16,
    pub version: String,
    pub shares: Vec<ShareStatus>,
    pub inbox: Option<PathBuf>,
    pub devices: Vec<Device>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "r", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Error { message: String },
    Status(Status),
    /// A pair window is open until the connection closes or it times out.
    PairOpen { secs: u64 },
    PairRequest { id: String, name: String, model: String, os: String, code: String },
    /// Whether the devices ended up paired.
    PairResult { name: String, paired: bool },
    PairClosed,
}

pub fn write_line(w: &mut impl Write, msg: &impl Serialize) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// The next message; `None` when the other side closed the connection.
pub fn read_line<T: DeserializeOwned>(r: &mut impl BufRead) -> Result<Option<T>> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(line.trim_end()).context("bad message")?))
}

/// A connection to the daemon.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Self> {
        let stream = UnixStream::connect(socket).map_err(|e| {
            let hint = match e.kind() {
                std::io::ErrorKind::PermissionDenied => " (run it with sudo)",
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                    " — is the service running? Try `sudo systemctl start rile-server`"
                }
                _ => "",
            };
            anyhow::anyhow!("can't reach rile-server at {}: {e}{hint}", socket.display())
        })?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        Ok(Self { reader: BufReader::new(stream.try_clone()?), writer: stream })
    }

    pub fn send(&mut self, req: &Request) -> Result<()> {
        write_line(&mut self.writer, req)?;
        Ok(())
    }

    pub fn recv(&mut self) -> Result<Option<Response>> {
        read_line(&mut self.reader)
    }

    pub fn set_timeout(&self, t: Option<Duration>) -> Result<()> {
        self.writer.set_read_timeout(t)?;
        Ok(())
    }

    /// One request, one answer; daemon errors become `Err`.
    pub fn call(socket: &Path, req: &Request) -> Result<Response> {
        let mut c = Self::connect(socket)?;
        c.send(req)?;
        match c.recv()? {
            Some(Response::Error { message }) => bail!(message),
            Some(r) => Ok(r),
            None => bail!("rile-server closed the connection"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_as_lines() {
        let mut buf = Vec::new();
        write_line(&mut buf, &Request::Pair { secs: 300 }).unwrap();
        write_line(&mut buf, &Response::PairResult { name: "Mac".into(), paired: true }).unwrap();
        assert_eq!(String::from_utf8_lossy(&buf).lines().next().unwrap(), r#"{"op":"pair","secs":300}"#);
        let mut r = &buf[..];
        assert_eq!(read_line::<Request>(&mut r).unwrap(), Some(Request::Pair { secs: 300 }));
        assert_eq!(read_line::<Response>(&mut r).unwrap(), Some(Response::PairResult { name: "Mac".into(), paired: true }));
        assert_eq!(read_line::<Response>(&mut r).unwrap(), None);
    }
}
