//! Install an official, checksum-verified llama.cpp runtime without a package manager.
//! Blocking: run on a worker. An existing installation is never overwritten.
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const RELEASES: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20";
const MAX_UNPACKED: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Deserialize)]
pub(crate) struct Release {
    pub(crate) tag_name: String,
    #[serde(default)]
    pub(crate) draft: bool,
    pub(crate) assets: Vec<Asset>,
}

#[derive(Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    pub(crate) size: u64,
    pub(crate) digest: Option<String>,
}

pub fn supported() -> bool {
    platform(std::env::consts::OS, std::env::consts::ARCH).is_ok()
}

fn platform(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Ok("macos-arm64.tar.gz"),
        ("macos", "x86_64") => Ok("macos-x64.tar.gz"),
        ("linux", "aarch64") => Ok("ubuntu-arm64.tar.gz"),
        ("linux", "x86_64") => Ok("ubuntu-x64.tar.gz"),
        ("windows", "aarch64") => Ok("win-cpu-arm64.zip"),
        ("windows", "x86_64") => Ok("win-cpu-x64.zip"),
        _ => bail!(
            "Automatic runtime installation is unavailable for {os}/{arch}. Set a custom llama-server path instead."
        ),
    }
}

fn select<'a>(releases: &'a [Release], suffix: &str) -> Result<(String, &'a Asset, String)> {
    for release in releases.iter().filter(|r| !r.draft) {
        // Some stable releases contain only metadata; actual builds can be prereleases.
        let Some(asset) = release
            .assets
            .iter()
            .find(|a| a.name == format!("llama-{}-bin-{suffix}", release.tag_name))
        else {
            continue;
        };
        let tag = &release.tag_name;
        if tag.is_empty()
            || !tag
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || tag == "."
            || tag == ".."
        {
            bail!("Invalid runtime release tag");
        }
        let expected_url = format!(
            "https://github.com/ggml-org/llama.cpp/releases/download/{tag}/{}",
            asset.name
        );
        if asset.browser_download_url != expected_url
            || asset.size == 0
            || asset.size > 512 * 1024 * 1024
        {
            bail!("Invalid official runtime asset");
        }
        let hash = asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .context(
                "Official runtime has no SHA-256 digest; refusing to install unverified code",
            )?;
        return Ok((
            format!(
                "{tag}-{}-{}",
                suffix.trim_end_matches(".tar.gz").trim_end_matches(".zip"),
                &hash[..12]
            ),
            asset,
            hash.to_owned(),
        ));
    }
    bail!(
        "No compatible official llama.cpp release found. You can still set a custom runtime path."
    )
}

pub(crate) fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Runtime installation cancelled");
    }
    Ok(())
}

#[derive(Debug)]
pub enum Progress {
    Stage(String),
    Download { done: u64, total: u64 },
}

pub fn install(cancel: &AtomicBool, progress: impl FnMut(Progress)) -> Result<PathBuf> {
    let root = rile_core::settings::Settings::data_dir()
        .context("Application data directory")?
        .join("runtimes");
    install_in(&root, cancel, progress)
}

/// The llama-server to run for a configured path. The bare default name would
/// pick up whatever build is on PATH (often an older package-manager build), so
/// the newest runtime installed by Rile takes precedence over it.
pub fn resolve(configured: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() && configured != "llama-server" {
        return configured.to_owned();
    }
    rile_core::settings::Settings::data_dir()
        .and_then(|dir| newest_in(&dir.join("runtimes")))
        .map_or_else(|| "llama-server".to_owned(), |path| path.to_string_lossy().into_owned())
}

/// The installed runtime with the highest llama.cpp build number (`b<build>-…` folders).
fn newest_in(root: &Path) -> Option<PathBuf> {
    fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            let build: u64 = name.strip_prefix('b')?.split('-').next()?.parse().ok()?;
            Some((build, item.path()))
        })
        .filter(|(_, path)| path.is_dir())
        .max_by_key(|(build, _)| *build)
        .and_then(|(_, dir)| find_server(&dir).ok())
}

fn install_in(
    root: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<PathBuf> {
    cancelled(cancel)?;
    let suffix = platform(std::env::consts::OS, std::env::consts::ARCH)?;
    progress(Progress::Stage(
        "Finding an official llama.cpp runtime…".into(),
    ));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent("rile-runtime-installer")
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let json = agent
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("Check official llama.cpp releases (GitHub may be rate limiting requests)")?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()?;
    let releases: Vec<Release> = serde_json::from_str(&json)?;
    let (version, asset, digest) = select(&releases, suffix)?;
    cancelled(cancel)?;
    fs::create_dir_all(root)?;
    let root = dunce::canonicalize(root)?;
    let destination = root.join(version);
    if destination.exists() {
        let executable = find_server(&destination)?;
        check_executable(&executable, cancel)?;
        return Ok(executable);
    }
    // Same filesystem as the destination for atomic publication. Drop removes partials.
    let stage = tempfile::tempdir_in(&root)?;
    let mut last = Instant::now();
    let archive = crate::updater::download(
        &crate::updater::Asset {
            name: asset.name.clone(),
            download_url: asset.browser_download_url.clone(),
            size: asset.size,
            checksum_url: None,
        },
        stage.path(),
        cancel,
        |done, total| {
            if done == 0 || done == total || last.elapsed() >= Duration::from_millis(250) {
                progress(Progress::Download { done, total });
                last = Instant::now();
            }
        },
    )?;
    cancelled(cancel)?;
    progress(Progress::Stage("Verifying runtime SHA-256…".into()));
    verify(&archive, &digest)?;
    let unpacked = stage.path().join("runtime");
    fs::create_dir(&unpacked)?;
    progress(Progress::Stage(
        "Extracting runtime and checking llama-server…".into(),
    ));
    extract(&archive, &unpacked, cancel)?;
    let executable = find_server(&unpacked)?;
    check_executable(&executable, cancel)?;
    let relative = executable.strip_prefix(&unpacked)?.to_owned();
    cancelled(cancel)?;
    // Concurrent installers may have completed the same version in the meantime.
    if destination.exists() {
        let executable = find_server(&destination)?;
        check_executable(&executable, cancel)?;
        return Ok(executable);
    }
    fs::rename(&unpacked, &destination).context("Publish runtime installation")?;
    Ok(destination.join(relative))
}

pub(crate) fn verify(archive: &Path, digest: &str) -> Result<()> {
    if !crate::updater::sha256_file(archive)?.eq_ignore_ascii_case(digest) {
        bail!("Runtime checksum mismatch; nothing was installed");
    }
    Ok(())
}

pub(crate) fn safe_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

pub(crate) fn extract(archive: &Path, dest: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut total = 0u64;
    if archive.extension().is_some_and(|e| e == "zip") {
        let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
        if zip.len() > 10000 {
            bail!("Runtime archive has too many entries");
        }
        for i in 0..zip.len() {
            cancelled(cancel)?;
            let mut entry = zip.by_index(i)?;
            let path = entry
                .enclosed_name()
                .filter(|p| safe_path(p))
                .context("Unsafe runtime archive path")?;
            if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                bail!("Unexpected symlink in runtime zip");
            }
            total = total
                .checked_add(entry.size())
                .context("Archive size overflow")?;
            if total > MAX_UNPACKED {
                bail!("Runtime archive is too large");
            }
            let out = dest.join(path);
            if entry.is_dir() {
                fs::create_dir_all(out)?;
                continue;
            }
            fs::create_dir_all(out.parent().context("Archive parent directory")?)?;
            let mut file = fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut file)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(
                    out,
                    fs::Permissions::from_mode(entry.unix_mode().unwrap_or(0o644) & 0o777),
                )?;
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(archive)?));
        for (i, entry) in tar.entries()?.enumerate() {
            cancelled(cancel)?;
            if i >= 10000 {
                bail!("Runtime archive has too many entries");
            }
            let mut entry = entry?;
            if !safe_path(&entry.path()?) {
                bail!("Unsafe runtime archive path");
            }
            total = total
                .checked_add(entry.size())
                .context("Archive size overflow")?;
            if total > MAX_UNPACKED {
                bail!("Runtime archive is too large");
            }
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir() || kind.is_symlink() || kind.is_hard_link()) {
                bail!("Unexpected runtime archive entry");
            }
            if let Some(target) = entry.link_name()? {
                // Official runtime libraries use same-directory relative symlinks.
                if !safe_path(&target) {
                    bail!("Unsafe runtime archive link");
                }
            }
            if !entry.unpack_in(dest)? {
                bail!("Runtime archive path escaped its directory");
            }
        }
    }
    Ok(())
}

fn find_server(root: &Path) -> Result<PathBuf> {
    fn visit(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) -> Result<()> {
        if depth > 8 {
            bail!("Runtime archive nesting is too deep");
        }
        for item in fs::read_dir(dir)? {
            let item = item?;
            let kind = item.file_type()?;
            if kind.is_dir() {
                visit(&item.path(), depth + 1, found)?;
            } else if kind.is_file()
                && item.file_name()
                    == if cfg!(windows) {
                        "llama-server.exe"
                    } else {
                        "llama-server"
                    }
            {
                found.push(item.path());
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    visit(root, 0, &mut found)?;
    if found.len() != 1 {
        bail!("Expected one llama-server executable in the runtime archive");
    }
    Ok(found.remove(0))
}

fn check_executable(path: &Path, cancel: &AtomicBool) -> Result<()> {
    // Test dynamic libraries and OS compatibility before selecting the new runtime.
    let mut child = crate::no_window(&mut Command::new(path))
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Run installed llama-server")?;
    let start = Instant::now();
    let result = loop {
        if let Err(error) = cancelled(cancel) {
            break Err(error);
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => {
                break Err(anyhow::anyhow!(
                    "Installed llama-server could not start ({status}); use a custom runtime compatible with this OS"
                ));
            }
            Err(error) => break Err(error.into()),
            _ => {}
        }
        if start.elapsed() > Duration::from_secs(20) {
            break Err(anyhow::anyhow!("Installed runtime check timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn newest_installed_runtime_wins() {
        let dir = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
        for name in ["b9090-macos-arm64-aaa", "b11962-macos-arm64-bbb"] {
            let bin = dir.path().join(name).join("llama");
            fs::create_dir_all(&bin).unwrap();
            fs::write(bin.join(exe), "").unwrap();
        }
        fs::create_dir(dir.path().join("partial")).unwrap();
        assert!(newest_in(dir.path()).unwrap().starts_with(dir.path().join("b11962-macos-arm64-bbb")));
        assert_eq!(newest_in(&dir.path().join("missing")), None);
        assert_eq!(resolve("/opt/bin/llama-server"), "/opt/bin/llama-server");
    }
    #[test]
    fn platforms_and_verified_asset_selection() {
        assert_eq!(platform("macos", "aarch64").unwrap(), "macos-arm64.tar.gz");
        assert_eq!(platform("windows", "x86_64").unwrap(), "win-cpu-x64.zip");
        assert!(platform("linux", "riscv64").is_err());
        let json = serde_json::json!([
            {"tag_name":"v1", "assets":[]},
            {"tag_name":"b123", "assets":[{"name":"llama-b123-bin-macos-arm64.tar.gz",
                "browser_download_url":"https://github.com/ggml-org/llama.cpp/releases/download/b123/llama-b123-bin-macos-arm64.tar.gz",
                "size":42, "digest":format!("sha256:{}", "a".repeat(64))}]}
        ]);
        let mut releases: Vec<Release> = serde_json::from_value(json).unwrap();
        assert!(
            select(&releases, "macos-arm64.tar.gz")
                .unwrap()
                .0
                .starts_with("b123-")
        );
        releases[1].assets[0].digest = None;
        assert!(select(&releases, "macos-arm64.tar.gz").is_err());
        releases[1].assets[0].digest = Some(format!("sha256:{}", "b".repeat(64)));
        releases[1].assets[0].browser_download_url = "https://example.com/runtime".into();
        assert!(select(&releases, "macos-arm64.tar.gz").is_err());
    }

    #[test]
    fn rejects_bad_checksum_and_cancellation() {
        let file = tempfile::NamedTempFile::new().unwrap();
        fs::write(file.path(), b"not a runtime").unwrap();
        assert!(verify(file.path(), &"a".repeat(64)).is_err());
        verify(
            file.path(),
            &crate::updater::sha256_file(file.path()).unwrap(),
        )
        .unwrap();
        assert!(install_in(file.path(), &AtomicBool::new(true), |_| {}).is_err());
        assert!(!safe_path(Path::new("../escape")));
        assert!(!safe_path(Path::new("/absolute")));
    }

    #[test]
    fn extracts_zip_and_rejects_traversal() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("runtime.zip");
        for name in ["bin/llama-server", "../escape"] {
            let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"runtime").unwrap();
            zip.finish().unwrap();
            let out = tempfile::tempdir().unwrap();
            let result = extract(&archive, out.path(), &AtomicBool::new(false));
            assert_eq!(result.is_ok(), name == "bin/llama-server");
        }
    }

    #[test]
    fn extracts_tar_libraries_and_rejects_escaping_links() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("runtime.tar.gz");
        for target in ["libggml.so.1", "../outside"] {
            let encoder = flate2::write::GzEncoder::new(
                fs::File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(7);
            header.set_mode(0o755);
            header.set_cksum();
            let name = if cfg!(windows) {
                "bin/llama-server.exe"
            } else {
                "bin/llama-server"
            };
            tar.append_data(&mut header, name, &b"runtime"[..]).unwrap();
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            tar.append_link(&mut link, "libggml.so", target).unwrap();
            tar.into_inner().unwrap().finish().unwrap();
            let out = tempfile::tempdir().unwrap();
            // Symlink extraction is exercised only where symlink privileges are available.
            if cfg!(unix) {
                let result = extract(&archive, out.path(), &AtomicBool::new(false));
                assert_eq!(result.is_ok(), target == "libggml.so.1");
                if result.is_ok() {
                    assert_eq!(find_server(out.path()).unwrap(), out.path().join(name));
                    assert_eq!(
                        fs::read_link(out.path().join("libggml.so")).unwrap(),
                        Path::new(target)
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "downloads and executes the official llama.cpp runtime"]
    fn official_runtime_install_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let executable = install_in(dir.path(), &cancel, |s| eprintln!("{s:?}")).unwrap();
        assert!(executable.is_absolute() && executable.is_file());
        assert_eq!(install_in(dir.path(), &cancel, |_| {}).unwrap(), executable);
    }
}
