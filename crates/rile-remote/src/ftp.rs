//! FTP / FTPS (explicit TLS) backend built on `suppaftp` (sync) + `rustls`.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};
use suppaftp::list::{File as ListFile, ListParser, PosixPexQuery};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode, RustlsConnector, RustlsFtpStream, Status};

use crate::backend::{Backend, CHUNK};
use crate::{
    Auth, CONNECT_TIMEOUT, ConnectionInfo, ConnectionLost, HostKeyMismatch, IO_TIMEOUT, Protocol,
    RemoteEntry, known_hosts, paths,
};

const DATA_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) struct FtpBackend {
    ftp: RustlsFtpStream,
    mlsd: bool,
    mlst: bool,
    /// Whether `LIST -a` lists hidden (dot) entries here; None until probed.
    /// Servers like vsftpd hide them from a bare `LIST`, which made
    /// existing dot folders look missing.
    list_all: Option<bool>,
    login_dir: String,
    alive: bool,
}

/// Map a suppaftp error, classifying transport failures as [`ConnectionLost`].
fn map_err(e: FtpError) -> anyhow::Error {
    match e {
        FtpError::ConnectionError(io) => ConnectionLost(io.to_string()).into(),
        FtpError::UnexpectedResponse(ref r) if closes_connection(r.status) => {
            ConnectionLost(response_text(r)).into()
        }
        FtpError::UnexpectedResponse(r) => {
            anyhow!("server replied {}: {}", r.status.code(), response_text(&r))
        }
        other => anyhow!("FTP error: {other}"),
    }
}

/// 421 (service not available) and an unsolicited 221 (closing control connection,
/// e.g. idle timeout) mean the server has hung up.
fn closes_connection(status: Status) -> bool {
    matches!(status, Status::NotAvailable | Status::Closing)
}

fn response_text(r: &suppaftp::types::Response) -> String {
    let s = String::from_utf8_lossy(&r.body);
    let s = s.trim();
    // Strip the leading status code if present.
    s.get(3..)
        .filter(|_| s.len() > 3 && s.as_bytes()[..3].iter().all(u8::is_ascii_digit))
        .map(|rest| rest.trim_start_matches(['-', ' ']).trim().to_string())
        .unwrap_or_else(|| s.to_string())
}

pub(crate) fn connect_tcp(host: &str, port: u16) -> Result<TcpStream> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("could not resolve {host}"))?
        .collect();
    let mut last = None;
    for addr in &addrs {
        match TcpStream::connect_timeout(addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                s.set_read_timeout(Some(IO_TIMEOUT))?;
                s.set_write_timeout(Some(IO_TIMEOUT))?;
                s.set_nodelay(true).ok();
                return Ok(s);
            }
            Err(e) => last = Some(e),
        }
    }
    match last {
        Some(e) => Err(e).with_context(|| format!("could not connect to {host}:{port}")),
        None => bail!("{host} resolved to no addresses"),
    }
}

impl FtpBackend {
    pub(crate) fn connect(info: &ConnectionInfo, secret: Option<&str>) -> Result<Self> {
        let tcp = connect_tcp(&info.host, info.port)?;
        let ipv6 = tcp.peer_addr().map(|a| a.is_ipv6()).unwrap_or(false);
        let mut ftp = RustlsFtpStream::connect_with_stream(tcp)
            .map_err(map_err)
            .context("FTP greeting failed")?
            .passive_stream_builder(|addr| {
                let s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
                    .map_err(FtpError::ConnectionError)?;
                s.set_read_timeout(Some(DATA_TIMEOUT)).ok();
                s.set_write_timeout(Some(DATA_TIMEOUT)).ok();
                Ok(s)
            });
        if ipv6 {
            ftp.set_mode(Mode::ExtendedPassive);
        } else {
            ftp.set_mode(Mode::Passive);
            // Servers behind NAT often advertise private addresses in PASV replies.
            ftp.set_passive_nat_workaround(true);
        }
        if info.protocol == Protocol::Ftps {
            let config = tls_config(&info.host, info.port)?;
            ftp = ftp
                .into_secure(RustlsConnector::from(config), &info.host)
                .map_err(|e| match e {
                    FtpError::SecureError(msg) => anyhow!("TLS negotiation failed: {msg}"),
                    other => map_err(other),
                })
                .context("could not enable TLS (AUTH TLS)")?;
        }
        let (user, pass) = match &info.auth {
            Auth::Password => (info.username.clone(), secret.unwrap_or("").to_string()),
            Auth::Anonymous => (
                if info.username.is_empty() {
                    "anonymous".to_string()
                } else {
                    info.username.clone()
                },
                secret.unwrap_or("anonymous@").to_string(),
            ),
            Auth::KeyFile { .. } | Auth::Agent | Auth::OAuth { .. } => {
                bail!("key file / agent authentication is only supported for SFTP")
            }
        };
        ftp.login(user.as_str(), pass.as_str())
            .map_err(|e| match e {
                FtpError::UnexpectedResponse(r) => {
                    anyhow!("login failed: {} {}", r.status.code(), response_text(&r))
                }
                other => map_err(other),
            })?;
        ftp.transfer_type(FileType::Binary).map_err(map_err)?;
        let (mlsd, mlst) = match ftp.feat() {
            Ok(f) => {
                let has = |k: &str| f.keys().any(|key| key.trim().eq_ignore_ascii_case(k));
                (has("MLSD"), has("MLST"))
            }
            Err(_) => (false, false),
        };
        let login_dir = ftp
            .pwd()
            .map(|p| paths::normalize(&p))
            .unwrap_or_else(|_| "/".into());
        Ok(Self {
            ftp,
            mlsd,
            mlst,
            list_all: None,
            login_dir,
            alive: true,
        })
    }

    /// Run a control-channel command, marking the connection dead on transport errors.
    fn cmd<T>(&mut self, f: impl FnOnce(&mut RustlsFtpStream) -> Result<T, FtpError>) -> Result<T> {
        f(&mut self.ftp).map_err(|e| {
            let e = map_err(e);
            if crate::is_connection_lost(&e) {
                self.alive = false;
            }
            e
        })
    }

    /// Stop a download early. Rather than ABOR (whose reply sequence differs between
    /// servers and easily desynchronises the control channel), close the data connection
    /// and read the single completion reply (226, or 426/451 for the broken transfer),
    /// then verify with NOOP that the control channel is in sync. If anything is off,
    /// the connection is marked dead and will be re-established on next use.
    fn stop_transfer<T: suppaftp::TlsStream>(&mut self, stream: suppaftp::TransferStream<T>) {
        match stream.finish() {
            Ok(()) | Err(FtpError::UnexpectedResponse(_)) => {}
            Err(_) => {
                self.alive = false;
                return;
            }
        }
        if self
            .ftp
            .custom_command("NOOP", &[Status::CommandOk])
            .is_err()
        {
            self.alive = false;
        }
    }

    #[cfg(test)]
    pub(crate) fn force_list_fallback(&mut self) {
        self.mlsd = false;
        self.mlst = false;
    }

    fn list_fallback(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        // CWD + bare LIST is more portable than `LIST <path>` (spaces, globbing).
        self.cmd(|f| f.cwd(path))?;
        let lines = match self.list_all {
            Some(true) => self.cmd(|f| f.list(Some("-a")))?,
            Some(false) => self.cmd(|f| f.list(None))?,
            None => {
                // Probe: `-a` must give at least everything a bare LIST does
                // (some servers would take it for a file name). Only a
                // non-empty folder can tell; empty ones probe again later.
                let plain = self.cmd(|f| f.list(None))?;
                let names = |ls: &[String]| -> Vec<String> { ls.iter().filter_map(|l| parse_list_line(l)).map(|(f, _)| f.name().to_string()).collect() };
                let plain_names = names(&plain);
                if plain_names.is_empty() {
                    plain
                } else {
                    let all = self.ftp.list(Some("-a")).ok();
                    let ok = all.as_ref().is_some_and(|all| {
                        let all_names = names(all);
                        plain_names.iter().all(|n| all_names.contains(n))
                    });
                    self.list_all = Some(ok);
                    match all {
                        Some(all) if ok => all,
                        _ => plain,
                    }
                }
            }
        };
        let _ = self.cmd(|f| f.cwd("/"));
        Ok(lines
            .iter()
            .filter_map(|l| parse_list_line(l))
            .map(|(f, src)| list_entry(path, &f, src))
            .collect())
    }
}

/// Where a parsed listing line came from (decides which fields are meaningful).
#[derive(Clone, Copy)]
pub(crate) enum Source<'a> {
    Posix,
    Dos,
    /// MLSD/MLST with the (lowercased) facts that were actually present.
    Mlsx(&'a str),
}

/// Parse one `LIST` line (POSIX or DOS style); `None` for headers like `total 12`.
pub(crate) fn parse_list_line(line: &str) -> Option<(ListFile, Source<'static>)> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() || line.starts_with("total ") {
        return None;
    }
    let (f, src) = match ListParser::parse_posix(line) {
        Ok(f) => (f, Source::Posix),
        Err(_) => (ListParser::parse_dos(line).ok()?, Source::Dos),
    };
    match f.name() {
        "." | ".." | "" => None,
        _ => Some((f, src)),
    }
}

/// Parse one `MLSD` line; `None` for `.`/`..` (cdir/pdir) and unparseable lines.
pub(crate) fn parse_mlsd_line(dir: &str, line: &str) -> Option<RemoteEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let facts = line
        .split_once(' ')
        .map(|(f, _)| f)
        .unwrap_or("")
        .to_ascii_lowercase();
    if facts.contains("type=cdir;") || facts.contains("type=pdir;") {
        return None;
    }
    // Some servers (e.g. Pure-FTPd) report symlinks as `type=OS.unix=slink:<target>`,
    // which suppaftp rejects; rewrite the fact to the generic `type=link`.
    let rewritten;
    let line = match facts.find("type=os.unix=") {
        Some(start) => {
            let end = facts[start..].find(';').map_or(facts.len(), |i| start + i);
            rewritten = format!("{}type=link{}", &line[..start], &line[end..]);
            rewritten.as_str()
        }
        None => line,
    };
    let f = ListParser::parse_mlsd(line).ok()?;
    if matches!(f.name(), "." | ".." | "") {
        return None;
    }
    Some(list_entry(dir, &f, Source::Mlsx(&facts)))
}

/// Convert a parsed listing entry. suppaftp fills defaults for missing facts, so
/// `src` decides which fields are trustworthy.
fn list_entry(dir: &str, f: &ListFile, src: Source<'_>) -> RemoteEntry {
    let name = paths::file_name(f.name()).to_string();
    let has = |fact: &str| match src {
        Source::Posix => true,
        Source::Dos => fact == "modify=",
        Source::Mlsx(facts) => facts.contains(fact),
    };
    let modified = Some(f.modified()).filter(|t| *t != SystemTime::UNIX_EPOCH && has("modify="));
    let permissions = has("unix.mode=").then(|| {
        let mut mode = 0u32;
        for (i, who) in [
            PosixPexQuery::Owner,
            PosixPexQuery::Group,
            PosixPexQuery::Others,
        ]
        .into_iter()
        .enumerate()
        {
            let shift = 6 - 3 * i as u32;
            let bits = (u32::from(f.can_read(who)) << 2)
                | (u32::from(f.can_write(who)) << 1)
                | u32::from(f.can_execute(who));
            mode |= bits << shift;
        }
        mode
    });
    RemoteEntry {
        path: paths::join(dir, &name),
        name,
        is_dir: f.is_directory(),
        is_symlink: f.is_symlink(),
        size: if f.is_directory() { 0 } else { f.size() as u64 },
        modified,
        permissions,
    }
}

impl Backend for FtpBackend {
    fn login_dir(&mut self) -> Result<String> {
        Ok(self.login_dir.clone())
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>> {
        if self.mlsd {
            let lines = self.cmd(|f| f.mlsd(Some(path)))?;
            return Ok(lines
                .iter()
                .filter_map(|l| parse_mlsd_line(path, l))
                .collect());
        }
        self.list_fallback(path)
    }

    fn stat(&mut self, path: &str) -> Result<RemoteEntry> {
        if path == "/" {
            return Ok(RemoteEntry {
                name: String::new(),
                path: "/".into(),
                is_dir: true,
                is_symlink: false,
                size: 0,
                modified: None,
                permissions: None,
            });
        }
        if self.mlst {
            match self.ftp.mlst(Some(path)) {
                Ok(line) => {
                    let facts = line.split_once(' ').map(|(f, _)| f).unwrap_or("");
                    let facts = facts.to_ascii_lowercase();
                    if let Ok(f) = ListParser::parse_mlst(&line) {
                        let mut e = list_entry("/", &f, Source::Mlsx(&facts));
                        e.name = paths::file_name(path).to_string();
                        e.path = path.to_string();
                        return Ok(e);
                    }
                }
                Err(e @ FtpError::ConnectionError(_)) => {
                    self.alive = false;
                    return Err(map_err(e));
                }
                Err(FtpError::UnexpectedResponse(r)) if closes_connection(r.status) => {
                    self.alive = false;
                    return Err(ConnectionLost(response_text(&r)).into());
                }
                Err(FtpError::UnexpectedResponse(_)) => bail!("{path}: no such file or directory"),
                Err(_) => {} // fall back to listing the parent
            }
        }
        let parent = paths::parent(path).unwrap_or_else(|| "/".into());
        let name = paths::file_name(path);
        self.list(&parent)?
            .into_iter()
            .find(|e| e.name == name)
            .ok_or_else(|| anyhow!("{path}: no such file or directory"))
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        self.cmd(|f| f.mkdir(path))
            .with_context(|| format!("creating directory {path}"))
    }

    fn remove_file(&mut self, path: &str) -> Result<()> {
        self.cmd(|f| f.rm(path))
            .with_context(|| format!("deleting {path}"))
    }

    fn remove_dir(&mut self, path: &str) -> Result<()> {
        self.cmd(|f| f.rmdir(path))
            .with_context(|| format!("removing directory {path}"))
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.cmd(|f| f.rename(from, to))
            .with_context(|| format!("renaming {from} to {to}"))
    }

    fn read_file(&mut self, path: &str, sink: &mut dyn FnMut(&[u8]) -> Result<bool>) -> Result<()> {
        let mut stream = self.cmd(|f| f.retr_as_stream(path))?;
        let mut buf = vec![0u8; CHUNK];
        let outcome: Result<bool> = loop {
            match stream.read(&mut buf) {
                Ok(0) => break Ok(true),
                Ok(n) => match sink(&buf[..n]) {
                    Ok(true) => {}
                    Ok(false) => break Ok(false),
                    Err(e) => break Err(e),
                },
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => {
                    self.alive = false;
                    break Err(ConnectionLost(e.to_string()).into());
                }
            }
        };
        match outcome {
            Ok(true) => self.cmd(|_| stream.finish()),
            Ok(false) | Err(_) => {
                self.stop_transfer(stream);
                outcome.map(|_| ())
            }
        }
    }

    fn write_file(&mut self, path: &str, src: &mut dyn Read, _size: u64) -> Result<()> {
        let mut stream = self.cmd(|f| f.put_with_stream(path))?;
        let mut buf = vec![0u8; CHUNK];
        let outcome: Result<()> = loop {
            match src.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    if let Err(e) = stream.write_all(&buf[..n]) {
                        break Err(anyhow::Error::new(e).context("data connection failed"));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => break Err(e.into()),
            }
        };
        if let Err(e) = stream.flush() {
            log::debug!("flush failed: {e}");
        }
        // Closing the data connection and reading the server's reply also tells us
        // why a write failed (quota, permissions...).
        let finished = self.cmd(|_| stream.finish());
        match (outcome, finished) {
            (Err(e), _) => Err(e),
            (Ok(()), r) => r,
        }
    }

    fn is_alive(&self) -> bool {
        self.alive
    }
}

// ------------------------------------------------------------------- FTPS TLS

fn tls_config(host: &str, port: u16) -> Result<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let webpki = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .context("building TLS verifier")?;
    let verifier = TofuVerifier {
        webpki,
        host: host.to_string(),
        port,
    };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("TLS configuration")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

pub(crate) const PIN_TYPE: &str = "x509-sha256";
pub(crate) const CA_TYPE: &str = "x509-ca";

/// Verifies against the Mozilla root store; certificates that fail CA validation
/// (typically self-signed ones) are accepted on first use and pinned by SHA-256
/// fingerprint in known_hosts, then rejected if they ever change. A host that once
/// presented a CA-valid certificate is never downgraded to TOFU.
#[derive(Debug)]
struct TofuVerifier {
    webpki: Arc<WebPkiServerVerifier>,
    host: String,
    port: u16,
}

fn fingerprint(cert: &[u8]) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, cert);
    d.as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn general(e: impl std::fmt::Display) -> rustls::Error {
    rustls::Error::General(e.to_string())
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let (host, port) = (self.host.as_str(), self.port);
        match self.webpki.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Ok(v) => {
                if !known_hosts::has(host, port, CA_TYPE).unwrap_or(true) {
                    let _ = known_hosts::add(host, port, CA_TYPE, "verified");
                }
                Ok(v)
            }
            Err(rustls::Error::InvalidCertificate(reason)) => {
                if known_hosts::has(host, port, CA_TYPE).map_err(general)? {
                    return Err(rustls::Error::InvalidCertificate(reason));
                }
                if matches!(reason, CertificateError::Revoked) {
                    return Err(rustls::Error::InvalidCertificate(reason));
                }
                let fp = fingerprint(end_entity.as_ref());
                match known_hosts::check(host, port, PIN_TYPE, &fp).map_err(general)? {
                    known_hosts::Check::Match => Ok(ServerCertVerified::assertion()),
                    known_hosts::Check::Unknown => {
                        log::warn!(
                            "accepting untrusted certificate for {host}:{port} on first use ({reason:?}); pinned {fp}"
                        );
                        known_hosts::add(host, port, PIN_TYPE, &fp).map_err(general)?;
                        Ok(ServerCertVerified::assertion())
                    }
                    known_hosts::Check::Mismatch { expected } => Err(general(HostKeyMismatch {
                        host: format!("{host}:{port}"),
                        expected: format!("certificate SHA256 {expected}"),
                        actual: format!("certificate SHA256 {fp}"),
                        known_hosts: known_hosts::path(),
                    })),
                }
            }
            Err(e) => Err(e),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.webpki.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mlsd_lines() {
        let dir = "/home/u";
        assert!(parse_mlsd_line(dir, "type=cdir;modify=20200101000000; .").is_none());
        assert!(parse_mlsd_line(dir, "type=pdir;modify=20200101000000; ..").is_none());
        let e = parse_mlsd_line(
            dir,
            "type=file;size=1234;modify=20201019151930;UNIX.mode=0644; my file;x.txt",
        )
        .unwrap();
        assert_eq!(e.name, "my file;x.txt");
        assert_eq!(e.path, "/home/u/my file;x.txt");
        assert!(!e.is_dir && !e.is_symlink);
        assert_eq!(e.size, 1234);
        assert_eq!(e.permissions, Some(0o644));
        let t = e
            .modified
            .unwrap()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        assert_eq!(t.as_secs(), 1603120770);

        let d = parse_mlsd_line(dir, "type=dir;modify=20201019151930; sub").unwrap();
        assert!(d.is_dir);
        assert_eq!(d.permissions, None);
        assert_eq!(d.size, 0);

        let n = parse_mlsd_line(dir, "type=file;size=5; nomtime").unwrap();
        assert_eq!(n.modified, None);

        let l =
            parse_mlsd_line(dir, "type=OS.unix=slink:/etc/x;modify=20201019151930; lnk").unwrap();
        assert!(l.is_symlink);
        assert_eq!(l.name, "lnk");
        assert!(l.modified.is_some());
    }

    #[test]
    fn list_lines() {
        assert!(parse_list_line("total 12").is_none());
        assert!(parse_list_line("drwxr-xr-x 2 u g 4096 Nov 5 2018 .").is_none());
        let (f, src) =
            parse_list_line("-rw-r--r-- 1 user group 1234 Nov 5 2018 example file.txt").unwrap();
        let e = list_entry("/x", &f, src);
        assert_eq!(e.name, "example file.txt");
        assert_eq!(e.path, "/x/example file.txt");
        assert_eq!(e.size, 1234);
        assert_eq!(e.permissions, Some(0o644));
        assert!(e.modified.is_some());

        let (d, src) =
            parse_list_line("drwxr-x--- 2 user group 4096 Jan 1 12:00 docs\r\n").unwrap();
        let e = list_entry("/", &d, src);
        assert!(e.is_dir);
        assert_eq!(e.path, "/docs");
        assert_eq!(e.permissions, Some(0o750));

        let (l, src) =
            parse_list_line("lrwxrwxrwx 1 user group 7 Jan 1 12:00 link -> target").unwrap();
        let e = list_entry("/", &l, src);
        assert!(e.is_symlink);
        assert_eq!(e.name, "link");

        let (dos, src) = parse_list_line("04-27-00  09:09PM       <DIR>          DOS dir").unwrap();
        let e = list_entry("/", &dos, src);
        assert!(e.is_dir);
        assert_eq!(e.name, "DOS dir");
        assert_eq!(e.permissions, None);
        let (dos, src) =
            parse_list_line("04-14-00  03:47PM                  589 readme.htm").unwrap();
        assert_eq!(list_entry("/", &dos, src).size, 589);
    }

    /// Start an in-process anonymous FTP server rooted at `root`; returns its port.
    fn server(root: std::path::PathBuf) -> u16 {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                libunftp::ServerBuilder::new(Box::new(move || {
                    unftp_sbe_fs::Filesystem::new(root.clone()).unwrap()
                }))
                .passive_ports(40000..=49999)
                .build()
                .unwrap()
                .listen(format!("127.0.0.1:{port}"))
                .await
                .unwrap();
            });
        });
        for _ in 0..200 {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return port;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("server did not start");
    }

    fn anon(port: u16) -> ConnectionInfo {
        ConnectionInfo {
            id: "t".into(),
            name: "t".into(),
            protocol: Protocol::Ftp,
            host: "127.0.0.1".into(),
            port,
            username: String::new(),
            auth: Auth::Anonymous,
            initial_path: None,
        }
    }

    /// Servers like vsftpd hide dot entries from a bare LIST; the fallback
    /// asks for them with `LIST -a` once that's known to be understood.
    #[test]
    fn list_fallback_includes_dot_entries() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::write(root.path().join("x.txt"), b"x").unwrap();
        let port = server(root.path().to_path_buf());
        let mut b = FtpBackend::connect(&anon(port), None).unwrap();
        b.force_list_fallback();
        for _ in 0..2 {
            let mut names: Vec<String> = b.list("/").unwrap().into_iter().map(|e| e.name).collect();
            names.sort();
            assert_eq!(names, [".git", "x.txt"]);
        }
        assert!(b.list_all.is_some(), "probed on the first non-empty folder");
        assert!(b.stat("/.git").unwrap().is_dir);
    }

    #[test]
    fn list_fallback_and_early_stop_against_server() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("a dir")).unwrap();
        std::fs::write(root.path().join("a dir/f.bin"), vec![7u8; 3_000_000]).unwrap();
        std::fs::write(root.path().join("small.txt"), b"tiny").unwrap();
        let port = server(root.path().to_path_buf());
        let mut b = FtpBackend::connect(&anon(port), None).unwrap();
        assert!(b.mlsd, "libunftp advertises MLSD");
        let via_mlsd = b.list("/a dir").unwrap();
        b.force_list_fallback();
        let mut via_list = b.list("/").unwrap();
        via_list.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(via_list.len(), 2);
        assert_eq!(via_list[0].name, "a dir");
        assert!(via_list[0].is_dir);
        assert_eq!(via_list[1].name, "small.txt");
        assert_eq!(via_list[1].size, 4);
        let via_list_sub = b.list("/a dir").unwrap();
        assert_eq!(via_list_sub.len(), 1);
        assert_eq!(via_list_sub[0].path, via_mlsd[0].path);
        assert_eq!(via_list_sub[0].size, 3_000_000);
        assert_eq!(b.stat("/a dir/f.bin").unwrap().size, 3_000_000);
        // Early-terminated reads must leave the control channel in sync (no reconnect).
        for _ in 0..3 {
            let head = crate::backend::read_head(&mut b, "/a dir/f.bin", 10).unwrap();
            assert_eq!(head, vec![7u8; 10]);
            assert!(b.is_alive());
            let head = crate::backend::read_head(&mut b, "/small.txt", 2).unwrap();
            assert_eq!(head, b"ti");
            assert!(b.is_alive());
        }
        assert_eq!(b.list("/").unwrap().len(), 2);
        assert!(b.is_alive());
    }

    #[test]
    fn response_text_strips_code() {
        let r = suppaftp::types::Response::new(
            Status::FileUnavailable,
            b"550 No such file\r\n".to_vec(),
        );
        assert_eq!(response_text(&r), "No such file");
    }
}
