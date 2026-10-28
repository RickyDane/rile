//! Install the MLX runtime (`mlx_lm.server`) without Python, Homebrew or a terminal:
//! a checksum-verified `uv` from GitHub provisions a private Python and installs
//! the `mlx-lm` package into a virtual environment under `runtimes/`.
//! Blocking: run on a worker. Apple silicon Macs only.
use crate::model_runtime::{Progress, Release, cancelled, extract, safe_path, verify};
use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

const UV_RELEASE: &str = "https://api.github.com/repos/astral-sh/uv/releases/latest";
const UV_ASSET: &str = "uv-aarch64-apple-darwin.tar.gz";
/// Python version for the environment; MLX publishes wheels for it.
const PYTHON: &str = "3.12";
/// Virtual environments aren't relocatable, so this one is built in place.
const ENV_DIR: &str = "mlx-lm";

/// MLX runs on Apple silicon Macs only.
pub fn supported() -> bool {
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
}

/// The `mlx_lm.server` inside a runtime directory.
fn server_in(env: &Path) -> PathBuf {
    env.join("bin").join("mlx_lm.server")
}

/// Installs or upgrades `mlx-lm`; returns the `mlx_lm.server` executable.
pub fn install(cancel: &AtomicBool, progress: impl FnMut(Progress)) -> Result<PathBuf> {
    let root = rile_core::settings::Settings::data_dir()
        .context("Application data directory")?
        .join("runtimes");
    install_in(&root, cancel, progress)
}

fn install_in(root: &Path, cancel: &AtomicBool, mut progress: impl FnMut(Progress)) -> Result<PathBuf> {
    cancelled(cancel)?;
    if !supported() {
        bail!("MLX needs a Mac with Apple silicon.");
    }
    fs::create_dir_all(root)?;
    let root = dunce::canonicalize(root)?;
    let uv = install_uv(&root, cancel, &mut progress)?;
    let env = root.join(ENV_DIR);
    let server = server_in(&env);
    let fresh = !server.is_file();
    if fresh && env.exists() {
        // A previous attempt was interrupted.
        fs::remove_dir_all(&env).context("Remove incomplete MLX runtime")?;
    }
    let result = (|| {
        if fresh {
            progress(Progress::Stage("Installing Python for MLX…".into()));
            let mut venv = uv_command(&uv, &root);
            venv.args(["venv", "--python", PYTHON]).arg(&env);
            run(venv, cancel, Duration::from_secs(600), "Create the Python environment")?;
        }
        progress(Progress::Stage(
            if fresh { "Installing mlx-lm (a few minutes on first install)…" } else { "Updating mlx-lm…" }.into(),
        ));
        let mut pip = uv_command(&uv, &root);
        pip.args(["pip", "install", "--upgrade", "--python"])
            .arg(env.join("bin").join("python"))
            .arg("mlx-lm");
        run(pip, cancel, Duration::from_secs(1800), "Install mlx-lm")?;
        progress(Progress::Stage("Checking mlx_lm.server…".into()));
        let mut check = Command::new(&server);
        check.arg("--help");
        run(check, cancel, Duration::from_secs(120), "Run mlx_lm.server")
    })();
    if let Err(error) = result {
        if fresh {
            let _ = fs::remove_dir_all(&env);
        }
        return Err(error);
    }
    Ok(server)
}

/// `uv` confined to Rile's runtime folder: its own Python, no user config or cache.
fn uv_command(uv: &Path, root: &Path) -> Command {
    let mut command = Command::new(uv);
    command
        .env("UV_PYTHON_INSTALL_DIR", root.join("python"))
        .env("UV_PYTHON_PREFERENCE", "only-managed")
        .env("UV_NO_CONFIG", "1")
        .env("UV_NO_CACHE", "1")
        .env("UV_NO_PROGRESS", "1")
        .env_remove("VIRTUAL_ENV")
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH");
    command
}

/// Downloads the latest official `uv`, reusing an installed copy of that version.
fn install_uv(root: &Path, cancel: &AtomicBool, progress: &mut impl FnMut(Progress)) -> Result<PathBuf> {
    progress(Progress::Stage("Finding the uv installer…".into()));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent("rile-runtime-installer")
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let json = agent
        .get(UV_RELEASE)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("Check uv releases (GitHub may be rate limiting requests)")?
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()?;
    let release: Release = serde_json::from_str(&json)?;
    let (version, asset, digest) = select_uv(&release)?;
    let destination = root.join(version);
    if let Ok(uv) = find_uv(&destination) {
        return Ok(uv);
    }
    cancelled(cancel)?;
    // Same filesystem as the destination for atomic publication. Drop removes partials.
    let stage = tempfile::tempdir_in(root)?;
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
    progress(Progress::Stage("Verifying uv SHA-256…".into()));
    verify(&archive, &digest)?;
    let unpacked = stage.path().join("uv");
    fs::create_dir(&unpacked)?;
    extract(&archive, &unpacked, cancel)?;
    let relative = find_uv(&unpacked)?.strip_prefix(&unpacked)?.to_owned();
    if !destination.exists() {
        fs::rename(&unpacked, &destination).context("Publish uv installation")?;
    }
    Ok(destination.join(relative))
}

fn select_uv(release: &Release) -> Result<(String, &crate::model_runtime::Asset, String)> {
    let tag = &release.tag_name;
    if release.draft
        || tag.is_empty()
        || tag == "."
        || tag == ".."
        || !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        bail!("Invalid uv release");
    }
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == UV_ASSET)
        .context("The latest uv release has no Apple silicon build")?;
    let expected_url = format!("https://github.com/astral-sh/uv/releases/download/{tag}/{UV_ASSET}");
    if asset.browser_download_url != expected_url || asset.size == 0 || asset.size > 256 * 1024 * 1024 {
        bail!("Invalid official uv asset");
    }
    let hash = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .context("Official uv build has no SHA-256 digest; refusing to install unverified code")?;
    Ok((format!("uv-{tag}-{}", &hash[..12]), asset, hash.to_owned()))
}

/// The `uv` executable directly in `dir` or one level down (the archive's folder).
fn find_uv(dir: &Path) -> Result<PathBuf> {
    let direct = dir.join("uv");
    if direct.is_file() {
        return Ok(direct);
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path().join("uv");
        if path.is_file() && path.strip_prefix(dir).is_ok_and(safe_path) {
            return Ok(path);
        }
    }
    bail!("No uv executable in the installer archive")
}

/// Runs a step to completion; cancellation and timeouts kill it. Failures
/// include the end of its error output.
fn run(mut command: Command, cancel: &AtomicBool, limit: Duration, what: &str) -> Result<()> {
    let log = tempfile::NamedTempFile::new()?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log.reopen()?)
        .spawn()
        .with_context(|| format!("{what}: could not start"))?;
    let start = Instant::now();
    let result = loop {
        if let Err(error) = cancelled(cancel) {
            break Err(error);
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(status)) => {
                let output = fs::read_to_string(log.path()).unwrap_or_default();
                let tail: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).rev().take(3).collect();
                let tail: Vec<&str> = tail.into_iter().rev().collect();
                break Err(anyhow::anyhow!("{what} failed ({status}). {}", tail.join(" ")));
            }
            Err(error) => break Err(error.into()),
            Ok(None) => {}
        }
        if start.elapsed() > limit {
            break Err(anyhow::anyhow!("{what} timed out"));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::atomic::Ordering;

    #[test]
    fn selects_only_verified_official_uv() {
        let json = serde_json::json!({"tag_name": "0.9.1", "assets": [
            {"name": UV_ASSET, "size": 42, "digest": format!("sha256:{}", "a".repeat(64)),
             "browser_download_url": format!("https://github.com/astral-sh/uv/releases/download/0.9.1/{UV_ASSET}")},
            {"name": "uv-x86_64-apple-darwin.tar.gz", "size": 42, "digest": null,
             "browser_download_url": "https://github.com/astral-sh/uv/releases/download/0.9.1/uv-x86_64-apple-darwin.tar.gz"}]});
        let mut release: Release = serde_json::from_value(json).unwrap();
        assert_eq!(select_uv(&release).unwrap().0, "uv-0.9.1-aaaaaaaaaaaa");
        release.assets[0].digest = None;
        assert!(select_uv(&release).is_err());
        release.assets[0].digest = Some(format!("sha256:{}", "b".repeat(64)));
        release.assets[0].browser_download_url = "https://example.com/uv.tar.gz".into();
        assert!(select_uv(&release).is_err());
        release.assets[0].browser_download_url =
            format!("https://github.com/astral-sh/uv/releases/download/0.9.1/{UV_ASSET}");
        release.tag_name = "../x".into();
        assert!(select_uv(&release).is_err());
    }

    #[test]
    fn finds_uv_in_archive_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_uv(dir.path()).is_err());
        fs::create_dir(dir.path().join("uv-aarch64-apple-darwin")).unwrap();
        fs::write(dir.path().join("uv-aarch64-apple-darwin/uv"), b"bin").unwrap();
        assert_eq!(find_uv(dir.path()).unwrap(), dir.path().join("uv-aarch64-apple-darwin/uv"));
    }

    #[cfg(unix)]
    #[test]
    fn steps_report_errors_and_cancel() {
        let cancel = AtomicBool::new(false);
        run(Command::new("true"), &cancel, Duration::from_secs(10), "ok").unwrap();
        let mut failing = Command::new("sh");
        failing.args(["-c", "echo 'No matching distribution' >&2; exit 3"]);
        let error = run(failing, &cancel, Duration::from_secs(10), "Install").unwrap_err().to_string();
        assert!(error.contains("No matching distribution"), "{error}");
        let mut slow = Command::new("sleep");
        slow.arg("30");
        assert!(run(slow, &cancel, Duration::from_millis(300), "Wait").unwrap_err().to_string().contains("timed out"));
        cancel.store(true, Ordering::Relaxed);
        assert!(run(Command::new("true"), &cancel, Duration::from_secs(10), "ok").is_err());
    }

    #[test]
    #[ignore = "downloads uv, Python and mlx-lm"]
    fn official_mlx_runtime_install_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let server = install_in(dir.path(), &cancel, |p| eprintln!("{p:?}")).unwrap();
        assert!(server.is_file());
        assert_eq!(install_in(dir.path(), &cancel, |_| {}).unwrap(), server);
    }
}
