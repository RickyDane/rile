//! Trust-on-first-use store for SSH host keys and FTPS certificate pins.
//!
//! File format (OpenSSH-compatible for SSH keys): one `pattern keytype data` per line,
//! where `pattern` is `host` for port 22 and `[host]:port` otherwise. FTPS entries use
//! the key types `x509-sha256` (pinned self-signed/untrusted certificate fingerprint)
//! and `x509-ca` (host previously presented a CA-verified certificate).
//! Default location: `dirs::config_dir()/rile/known_hosts`.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};

static OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// Path of the known_hosts file in use.
pub fn path() -> PathBuf {
    if let Some(p) = OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return p;
    }
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("rile")
        .join("known_hosts")
}

/// Use a different known_hosts file for this process (e.g. for tests).
pub fn set_path_override(path: Option<PathBuf>) {
    *OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

/// `host` for port 22, `[host]:port` otherwise (OpenSSH convention).
pub fn host_pattern(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Check {
    Match,
    Unknown,
    Mismatch { expected: String },
}

fn read_lines() -> Result<Vec<String>> {
    match fs::read_to_string(path()) {
        Ok(s) => Ok(s.lines().map(str::to_string).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path().display())),
    }
}

fn parse(line: &str) -> Option<(&str, &str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut it = line.split_whitespace();
    let pat = it.next()?;
    let ty = it.next()?;
    let data = it.next()?;
    Some((pat, ty, data))
}

fn pattern_matches(patterns: &str, wanted: &str) -> bool {
    patterns.split(',').any(|p| p == wanted)
}

/// Look up `(host, port, key_type)` and compare with `data`.
pub(crate) fn check(host: &str, port: u16, key_type: &str, data: &str) -> Result<Check> {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let wanted = host_pattern(host, port);
    let mut expected = None;
    for line in read_lines()? {
        if let Some((pat, ty, d)) = parse(&line)
            && ty == key_type
            && pattern_matches(pat, &wanted)
        {
            if d == data {
                return Ok(Check::Match);
            }
            expected = Some(d.to_string());
        }
    }
    Ok(match expected {
        Some(expected) => Check::Mismatch { expected },
        None => Check::Unknown,
    })
}

/// Whether any entry of `key_type` exists for the host.
pub(crate) fn has(host: &str, port: u16, key_type: &str) -> Result<bool> {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let wanted = host_pattern(host, port);
    Ok(read_lines()?.iter().any(|l| {
        parse(l).is_some_and(|(pat, ty, _)| ty == key_type && pattern_matches(pat, &wanted))
    }))
}

/// Append an entry.
pub(crate) fn add(host: &str, port: u16, key_type: &str, data: &str) -> Result<()> {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = path();
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .with_context(|| format!("opening {}", p.display()))?;
    writeln!(f, "{} {} {}", host_pattern(host, port), key_type, data)
        .with_context(|| format!("writing {}", p.display()))?;
    Ok(())
}

/// Remove every entry for `host:port` (e.g. after the user accepted a changed key).
pub fn forget(host: &str, port: u16) -> Result<()> {
    let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let wanted = host_pattern(host, port);
    let lines = read_lines()?;
    let kept: Vec<&String> = lines
        .iter()
        .filter(|l| !parse(l).is_some_and(|(pat, _, _)| pattern_matches(pat, &wanted)))
        .collect();
    if kept.len() == lines.len() {
        return Ok(());
    }
    let mut out = String::new();
    for l in kept {
        out.push_str(l);
        out.push('\n');
    }
    fs::write(path(), out).with_context(|| format!("writing {}", path().display()))
}
