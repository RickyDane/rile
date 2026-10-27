//! Self-updater backed by GitHub releases.
//!
//! Flow: [`check`] → [`download`] (with optional SHA-256 verification) →
//! [`install`]. All calls are blocking; run them on a background thread.
//!
//! Expected release assets are named with the Rust target triple, e.g.
//! `rile-aarch64-apple-darwin.tar.gz` (or `.zip`, or a raw binary), and an
//! optional macOS bundle `rile-aarch64-apple-darwin.app.zip`. Checksums may be
//! provided per asset (`<asset>.sha256`) or as one `SHA256SUMS` /
//! `checksums.txt` file, in `sha256sum` format (`<hex>  <name>`).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const USER_AGENT: &str = concat!("rile-updater/", env!("CARGO_PKG_VERSION"));
const API_BASE: &str = "https://api.github.com";
const MAX_JSON_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateConfig {
    /// `"owner/name"`.
    pub repo: String,
    pub current_version: String,
    pub allow_prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Normalised version (no leading `v`).
    pub version: String,
    pub notes: String,
    /// Release page URL.
    pub url: String,
    /// Asset for this platform, if the release has one.
    pub asset: Option<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub download_url: String,
    pub size: u64,
    pub checksum_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub restart_required: bool,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

// ---------------------------------------------------------------- platform

/// Rust target triple of this build, e.g. `aarch64-apple-darwin`.
pub fn target_triple() -> String {
    let arch = std::env::consts::ARCH;
    let rest = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else if cfg!(target_os = "windows") {
        if cfg!(target_env = "gnu") { "pc-windows-gnu" } else { "pc-windows-msvc" }
    } else if cfg!(target_os = "linux") {
        if cfg!(target_env = "musl") { "unknown-linux-musl" } else { "unknown-linux-gnu" }
    } else {
        std::env::consts::OS
    };
    format!("{arch}-{rest}")
}

/// Preferred asset name for this platform, e.g.
/// `rile-aarch64-apple-darwin.tar.gz` (`.zip` on Windows).
pub fn asset_name_for_platform() -> String {
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!("rile-{}.{ext}", target_triple())
}

/// The `.app` bundle containing the running executable, if any.
pub fn current_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    app_bundle_of(&exe)
}

fn app_bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is_app = app.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"));
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app).then(|| app.to_path_buf())
}

// ---------------------------------------------------------------- versions

/// Parses a version, tolerating a leading `v` and missing minor/patch parts.
pub fn parse_version(s: &str) -> Option<semver::Version> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    if let Ok(v) = semver::Version::parse(s) {
        return Some(v);
    }
    // "1.2" / "1" / "1.2-beta": pad the numeric core.
    let (core, rest) = match s.find(['-', '+']) {
        Some(i) => s.split_at(i),
        None => (s, ""),
    };
    let parts = core.split('.').count();
    if parts == 0 || parts > 3 {
        return None;
    }
    let padded = format!("{core}{}{rest}", ".0".repeat(3 - parts));
    semver::Version::parse(&padded).ok()
}

/// True if `candidate` is strictly newer than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

// ---------------------------------------------------------------- selection

fn is_checksum_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".sha256") || n.ends_with(".sha256sum") || n.contains("sha256sums") || n.contains("checksums")
}

/// Picks the platform asset from a release. Prefers an `.app.zip` bundle
/// when `prefer_bundle`, then the exact expected name, then any non-checksum
/// asset whose name contains the triple.
fn select_asset(assets: &[GhAsset], triple: &str, prefer_bundle: bool) -> Option<Asset> {
    let expected = format!("rile-{triple}.{}", if triple.contains("windows") { "zip" } else { "tar.gz" });
    let candidates: Vec<&GhAsset> =
        assets.iter().filter(|a| a.name.contains(triple) && !is_checksum_name(&a.name)).collect();
    let is_bundle = |a: &&GhAsset| a.name.ends_with(".app.zip");
    let score = |a: &&GhAsset| -> u8 {
        match (prefer_bundle && is_bundle(a), a.name == expected, is_bundle(a)) {
            (true, _, _) => 0,
            (_, true, _) => 1,
            (_, _, false) => 2,
            _ => 3,
        }
    };
    let best = candidates.into_iter().min_by_key(|a| score(a))?;
    let checksum_url = assets
        .iter()
        .find(|a| {
            a.name.eq_ignore_ascii_case(&format!("{}.sha256", best.name))
                || a.name.eq_ignore_ascii_case(&format!("{}.sha256sum", best.name))
        })
        .or_else(|| {
            assets.iter().find(|a| {
                let n = a.name.to_ascii_lowercase();
                n.contains("sha256sums") || n.contains("checksums")
            })
        })
        .map(|a| a.browser_download_url.clone());
    Some(Asset {
        name: best.name.clone(),
        download_url: best.browser_download_url.clone(),
        size: best.size,
        checksum_url,
    })
}

/// Picks the newest eligible release newer than `current`.
fn select_release(
    releases: &[GhRelease],
    current: &str,
    allow_prerelease: bool,
    triple: &str,
    prefer_bundle: bool,
) -> Option<Release> {
    let current = parse_version(current)?;
    releases
        .iter()
        .filter(|r| !r.draft && (allow_prerelease || !r.prerelease))
        .filter_map(|r| {
            let v = parse_version(&r.tag_name)?;
            if !allow_prerelease && !v.pre.is_empty() {
                return None;
            }
            (v > current).then_some((v, r))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(v, r)| Release {
            version: v.to_string(),
            notes: r.body.clone().or_else(|| r.name.clone()).unwrap_or_default(),
            url: r.html_url.clone(),
            asset: select_asset(&r.assets, triple, prefer_bundle),
        })
}

// ---------------------------------------------------------------- network

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .build()
        .into()
}

fn get_text(agent: &ureq::Agent, url: &str) -> Result<String> {
    let resp = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .with_context(|| format!("Request to {url} failed"))?;
    Ok(resp.into_body().into_with_config().limit(MAX_JSON_BYTES).read_to_string()?)
}

/// Returns the newest release newer than `cfg.current_version`, if any.
pub fn check(cfg: &UpdateConfig) -> Result<Option<Release>> {
    if !cfg.repo.contains('/') {
        bail!("Repository must be \"owner/name\"");
    }
    parse_version(&cfg.current_version)
        .ok_or_else(|| anyhow!("Invalid current version {:?}", cfg.current_version))?;
    let json = get_text(&agent(), &format!("{API_BASE}/repos/{}/releases?per_page=30", cfg.repo))?;
    let releases: Vec<GhRelease> = serde_json::from_str(&json).context("Unexpected GitHub API response")?;
    Ok(select_release(
        &releases,
        &cfg.current_version,
        cfg.allow_prerelease,
        &target_triple(),
        current_app_bundle().is_some(),
    ))
}

/// Downloads `asset` into `dest_dir`, reporting `(downloaded, total)` bytes,
/// and verifies its SHA-256 if the release has a checksum file. A partial
/// file is removed on cancel or error.
pub fn download(
    asset: &Asset,
    dest_dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<PathBuf> {
    let name = Path::new(&asset.name)
        .file_name()
        .filter(|n| *n == asset.name.as_str())
        .ok_or_else(|| anyhow!("Invalid asset name {:?}", asset.name))?;
    fs::create_dir_all(dest_dir)?;
    let dest = dest_dir.join(name);
    let part = dest_dir.join(format!("{}.part", asset.name));
    let agent = agent();

    let result = (|| -> Result<String> {
        let resp = agent
            .get(&asset.download_url)
            .header("Accept", "application/octet-stream")
            .call()
            .context("Download failed")?;
        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()?.parse().ok())
            .unwrap_or(asset.size);
        let mut reader = resp.into_body().into_reader();
        let mut file = fs::File::create(&part)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        on_progress(0, total);
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("Download cancelled");
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            done += n as u64;
            on_progress(done, total.max(done));
        }
        file.sync_all()?;
        Ok(to_hex(&hasher.finalize()))
    })();

    let actual = match result {
        Ok(h) => h,
        Err(e) => {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
    };
    if let Some(url) = &asset.checksum_url {
        let verified = get_text(&agent, url).and_then(|text| verify_checksum(&text, &asset.name, &actual));
        if let Err(e) = verified {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
    }
    fs::rename(&part, &dest)?;
    Ok(dest)
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Finds the expected hash for `name` in a `sha256sum`-style file (or a file
/// containing only a hash) and compares it to `actual_hex`.
pub fn verify_checksum(checksums: &str, name: &str, actual_hex: &str) -> Result<()> {
    let mut lines = checksums.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    let all: Vec<&str> = lines.by_ref().collect();
    let expected = all
        .iter()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            let hash = it.next()?;
            let file = it.next()?.trim_start_matches('*');
            let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
            (file == name).then_some(hash)
        })
        .or_else(|| match all.as_slice() {
            [only] if only.split_whitespace().count() == 1 => Some(*only),
            _ => None,
        })
        .ok_or_else(|| anyhow!("No checksum listed for {name}"))?;
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Malformed checksum for {name}");
    }
    if !expected.eq_ignore_ascii_case(actual_hex) {
        bail!("Checksum mismatch for {name}: expected {expected}, got {actual_hex}");
    }
    Ok(())
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(to_hex(&h.finalize()))
}

// ---------------------------------------------------------------- install

#[derive(Debug, PartialEq, Eq)]
enum ArchiveKind {
    TarGz,
    Zip,
    Raw,
}

fn archive_kind(path: &Path) -> ArchiveKind {
    let n = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if n.ends_with(".tar.gz") || n.ends_with(".tgz") {
        ArchiveKind::TarGz
    } else if n.ends_with(".zip") {
        ArchiveKind::Zip
    } else {
        ArchiveKind::Raw
    }
}

/// Installs a file produced by [`download`]: replaces the running app bundle
/// (macOS, when the archive contains a `.app`) or the running executable.
pub fn install(downloaded: &Path) -> Result<InstallOutcome> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let exe_name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let kind = archive_kind(downloaded);

    if kind == ArchiveKind::Raw {
        set_executable(downloaded)?;
        self_replace::self_replace(downloaded).context("Failed to replace executable")?;
        return Ok(replaced_msg(&exe));
    }

    // Extract next to the bundle when replacing one (same filesystem so the
    // final swap is a rename); otherwise next to the download.
    let bundle = app_bundle_of(&exe);
    let work_parent = bundle
        .as_ref()
        .and_then(|b| b.parent().map(Path::to_path_buf))
        .or_else(|| downloaded.parent().map(Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir);
    let work = tempfile::Builder::new().prefix(".rile-update-").tempdir_in(&work_parent)?;
    extract(downloaded, kind, work.path())?;

    if let Some(bundle) = &bundle
        && let Some(new_app) = find(work.path(), 2, &|p| {
            p.is_dir() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))
        })
    {
        swap_bundle(&new_app, bundle)?;
        return Ok(InstallOutcome {
            restart_required: true,
            message: format!("Updated {}. Restart to use the new version.", bundle.display()),
        });
    }

    let wanted = [exe_name.as_str(), if cfg!(windows) { "rile.exe" } else { "rile" }];
    let bin = find(work.path(), 4, &|p| {
        p.is_file() && p.file_name().is_some_and(|n| wanted.iter().any(|w| n == *w))
    })
    .ok_or_else(|| anyhow!("Archive does not contain a '{}' executable", wanted[1]))?;
    set_executable(&bin)?;
    self_replace::self_replace(&bin).context("Failed to replace executable")?;
    Ok(replaced_msg(&exe))
}

fn replaced_msg(exe: &Path) -> InstallOutcome {
    InstallOutcome {
        restart_required: true,
        message: format!("Updated {}. Restart to use the new version.", exe.display()),
    }
}

fn extract(archive: &Path, kind: ArchiveKind, dest: &Path) -> Result<()> {
    let file = fs::File::open(archive)?;
    match kind {
        ArchiveKind::TarGz => {
            let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(file));
            ar.set_preserve_permissions(true);
            ar.unpack(dest).context("Failed to extract tar.gz")?;
        }
        ArchiveKind::Zip => extract_zip(file, dest)?,
        ArchiveKind::Raw => unreachable!(),
    }
    Ok(())
}

fn extract_zip(file: fs::File, dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(file).context("Invalid zip archive")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else { bail!("Unsafe path in archive: {}", entry.name()) };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            fs::create_dir_all(p)?;
        }
        let mode = entry.unix_mode();
        #[cfg(unix)]
        if mode.is_some_and(|m| m & 0o170000 == 0o120000) {
            // Symlink (common inside .app frameworks): content is the target.
            let mut target = String::new();
            entry.read_to_string(&mut target)?;
            let t = Path::new(&target);
            let resolved = out.parent().map(|p| normalize(&p.join(t)));
            if t.is_absolute() || !resolved.is_some_and(|r| r.starts_with(dest)) {
                bail!("Unsafe symlink in archive: {}", entry.name());
            }
            std::os::unix::fs::symlink(t, &out)?;
            continue;
        }
        let mut f = fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&out, fs::Permissions::from_mode(m & 0o7777))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
    }
    Ok(())
}

/// Lexically resolves `.` and `..` components.
#[cfg(unix)]
fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Breadth-first search for the first path matching `pred`, up to `depth`.
fn find(root: &Path, depth: usize, pred: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut level = vec![root.to_path_buf()];
    for _ in 0..depth {
        let mut next = Vec::new();
        for dir in &level {
            let mut children: Vec<PathBuf> =
                fs::read_dir(dir).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
            children.sort();
            if let Some(hit) = children.iter().find(|p| pred(p)) {
                return Some(hit.clone());
            }
            next.extend(children.into_iter().filter(|p| {
                p.is_dir() && !p.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink())
            }));
        }
        level = next;
    }
    None
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(path)?.permissions();
        p.set_mode(p.mode() | 0o755);
        fs::set_permissions(path, p)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Replaces `old_bundle` with `new_bundle` (both in the same directory tree
/// on the same volume). On macOS this is an atomic `renamex_np(RENAME_SWAP)`;
/// elsewhere a rename-aside with rollback.
fn swap_bundle(new_bundle: &Path, old_bundle: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let a = CString::new(new_bundle.as_os_str().as_bytes())?;
        let b = CString::new(old_bundle.as_os_str().as_bytes())?;
        // SAFETY: both pointers are valid NUL-terminated C strings.
        let rc = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) };
        if rc == 0 {
            // `new_bundle` now holds the old app; it lives in the temp dir
            // and is removed with it.
            let _ = fs::remove_dir_all(new_bundle);
            return Ok(());
        }
        // Fall through (e.g. filesystem without swap support).
    }
    let parent = old_bundle.parent().ok_or_else(|| anyhow!("Bundle has no parent directory"))?;
    let aside = parent.join(format!(
        ".{}.old-{}",
        old_bundle.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    fs::rename(old_bundle, &aside).context("Failed to move old app aside (permission denied?)")?;
    if let Err(e) = fs::rename(new_bundle, old_bundle) {
        let _ = fs::rename(&aside, old_bundle);
        return Err(e).context("Failed to move new app into place");
    }
    let _ = fs::remove_dir_all(&aside);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {"tag_name":"v0.3.0-beta.1","name":"0.3 beta","body":"beta notes","html_url":"https://github.com/o/r/releases/tag/v0.3.0-beta.1",
       "draft":false,"prerelease":true,"assets":[
         {"name":"rile-aarch64-apple-darwin.tar.gz","browser_download_url":"https://dl/b/mac.tgz","size":10}]},
      {"tag_name":"v0.4.0","name":"draft","body":"","html_url":"x","draft":true,"prerelease":false,"assets":[]},
      {"tag_name":"v0.2.1","name":"0.2.1","body":"stable notes","html_url":"https://github.com/o/r/releases/tag/v0.2.1",
       "draft":false,"prerelease":false,"assets":[
         {"name":"rile-aarch64-apple-darwin.tar.gz","browser_download_url":"https://dl/mac-arm.tgz","size":111},
         {"name":"rile-aarch64-apple-darwin.app.zip","browser_download_url":"https://dl/mac-arm.app.zip","size":222},
         {"name":"rile-x86_64-unknown-linux-gnu.tar.gz","browser_download_url":"https://dl/linux.tgz","size":333},
         {"name":"rile-x86_64-unknown-linux-gnu.tar.gz.sha256","browser_download_url":"https://dl/linux.sha","size":1},
         {"name":"rile-x86_64-pc-windows-msvc.zip","browser_download_url":"https://dl/win.zip","size":444},
         {"name":"SHA256SUMS","browser_download_url":"https://dl/SUMS","size":1}]},
      {"tag_name":"0.1.0","body":null,"html_url":"y","assets":[]}
    ]"#;

    fn sample() -> Vec<GhRelease> {
        serde_json::from_str(SAMPLE).unwrap()
    }

    #[test]
    fn version_comparison() {
        assert!(is_newer("v1.2.0", "1.1.9"));
        assert!(is_newer("1.10.0", "v1.9.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(is_newer("v2", "1.9.9"));
        assert!(is_newer("1.3", "1.2.9"));
        assert!(!is_newer("garbage", "1.0.0"));
        assert_eq!(parse_version("v1.2-beta.2").unwrap().to_string(), "1.2.0-beta.2");
    }

    #[test]
    fn selects_newest_stable_release_and_asset() {
        let r = select_release(&sample(), "0.1.0", false, "aarch64-apple-darwin", false).unwrap();
        assert_eq!(r.version, "0.2.1");
        assert_eq!(r.notes, "stable notes");
        let a = r.asset.unwrap();
        assert_eq!(a.name, "rile-aarch64-apple-darwin.tar.gz");
        assert_eq!(a.size, 111);
        assert_eq!(a.checksum_url.as_deref(), Some("https://dl/SUMS"));

        // Inside an .app bundle the bundle zip wins.
        let r = select_release(&sample(), "0.1.0", false, "aarch64-apple-darwin", true).unwrap();
        assert_eq!(r.asset.unwrap().name, "rile-aarch64-apple-darwin.app.zip");

        // Per-asset checksum preferred over SHA256SUMS.
        let r = select_release(&sample(), "0.1.0", false, "x86_64-unknown-linux-gnu", false).unwrap();
        assert_eq!(r.asset.unwrap().checksum_url.as_deref(), Some("https://dl/linux.sha"));

        let r = select_release(&sample(), "v0.1.0", false, "x86_64-pc-windows-msvc", false).unwrap();
        assert_eq!(r.asset.unwrap().download_url, "https://dl/win.zip");

        // Unknown platform: release still reported, no asset.
        let r = select_release(&sample(), "0.1.0", false, "riscv64gc-unknown-linux-gnu", false).unwrap();
        assert!(r.asset.is_none());
    }

    #[test]
    fn prerelease_and_up_to_date() {
        let r = select_release(&sample(), "0.2.1", true, "aarch64-apple-darwin", false).unwrap();
        assert_eq!(r.version, "0.3.0-beta.1");
        assert_eq!(r.asset.unwrap().download_url, "https://dl/b/mac.tgz");
        assert!(select_release(&sample(), "0.2.1", false, "aarch64-apple-darwin", false).is_none());
        // Drafts are never offered.
        assert!(select_release(&sample(), "0.3.0", true, "aarch64-apple-darwin", false).is_none());
    }

    #[test]
    fn platform_names() {
        let n = asset_name_for_platform();
        assert!(n.starts_with("rile-") && n.contains(&target_triple()));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(n, "rile-aarch64-apple-darwin.tar.gz");
        assert_eq!(
            app_bundle_of(Path::new("/Applications/Rile.app/Contents/MacOS/rile")),
            Some(PathBuf::from("/Applications/Rile.app"))
        );
        assert_eq!(app_bundle_of(Path::new("/usr/local/bin/rile")), None);
    }

    #[test]
    fn checksum_verification() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("rile-x.tar.gz");
        fs::write(&f, "abc").unwrap();
        let h = sha256_file(&f).unwrap();
        assert_eq!(h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let sums = format!("# comment\n{}  other.zip\n{h}  rile-x.tar.gz\n", "0".repeat(64));
        verify_checksum(&sums, "rile-x.tar.gz", &h).unwrap();
        verify_checksum(&format!("{h} *./dist/rile-x.tar.gz"), "rile-x.tar.gz", &h).unwrap();
        verify_checksum(&format!("{}\n", h.to_uppercase()), "rile-x.tar.gz", &h).unwrap();
        let err = verify_checksum(&sums, "other.zip", &h).unwrap_err().to_string();
        assert!(err.contains("mismatch"), "{err}");
        assert!(verify_checksum(&sums, "missing", &h).is_err());
        assert!(verify_checksum("nothex  rile-x.tar.gz", "rile-x.tar.gz", &h).is_err());
    }

    #[test]
    fn extracts_archives_and_finds_binary() {
        let t = tempfile::tempdir().unwrap();
        // tar.gz with nested binary
        let tgz = t.path().join("rile-test.tar.gz");
        {
            let enc = flate2::write::GzEncoder::new(fs::File::create(&tgz).unwrap(), flate2::Compression::fast());
            let mut b = tar::Builder::new(enc);
            let mut h = tar::Header::new_gnu();
            h.set_size(4);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, "rile-1.0/rile", &b"\x7fELF"[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        assert_eq!(archive_kind(&tgz), ArchiveKind::TarGz);
        let out = t.path().join("x");
        fs::create_dir(&out).unwrap();
        extract(&tgz, ArchiveKind::TarGz, &out).unwrap();
        let bin = find(&out, 4, &|p| p.is_file() && p.file_name().is_some_and(|n| n == "rile")).unwrap();
        assert_eq!(bin, out.join("rile-1.0/rile"));

        // zip containing an .app bundle with a symlink
        let zp = t.path().join("rile.app.zip");
        {
            let mut z = zip::ZipWriter::new(fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
            z.add_directory("Rile.app/Contents/MacOS/", o).unwrap();
            z.start_file("Rile.app/Contents/MacOS/rile", o).unwrap();
            z.write_all(b"bin").unwrap();
            z.add_symlink("Rile.app/Contents/link", "MacOS/rile", o).unwrap();
            z.finish().unwrap();
        }
        assert_eq!(archive_kind(&zp), ArchiveKind::Zip);
        let out2 = t.path().join("y");
        fs::create_dir(&out2).unwrap();
        extract(&zp, ArchiveKind::Zip, &out2).unwrap();
        let app = find(&out2, 2, &|p| p.is_dir() && p.extension().is_some_and(|e| e == "app")).unwrap();
        assert_eq!(app, out2.join("Rile.app"));
        assert_eq!(fs::read(app.join("Contents/MacOS/rile")).unwrap(), b"bin");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = fs::metadata(app.join("Contents/MacOS/rile")).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o755);
            assert_eq!(fs::read_link(app.join("Contents/link")).unwrap(), Path::new("MacOS/rile"));
        }
    }

    #[test]
    fn bundle_swap() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Rile.app");
        fs::create_dir_all(old.join("Contents")).unwrap();
        fs::write(old.join("Contents/v"), "old").unwrap();
        let work = t.path().join(".work");
        let new = work.join("Rile.app");
        fs::create_dir_all(new.join("Contents")).unwrap();
        fs::write(new.join("Contents/v"), "new").unwrap();
        swap_bundle(&new, &old).unwrap();
        assert_eq!(fs::read_to_string(old.join("Contents/v")).unwrap(), "new");
        assert!(!new.exists());
        let leftovers: Vec<_> = fs::read_dir(t.path()).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(leftovers.len(), 2, "only Rile.app and .work should remain");
    }
}
