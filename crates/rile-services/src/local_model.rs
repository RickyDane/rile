//! Local models — GGUF files run by the llama.cpp server, and MLX model
//! folders (Hugging Face snapshots) run by `mlx_lm.server` — plus their
//! downloads. All operations block: callers must use a worker thread.
use anyhow::{Context, Result, bail};
use rile_core::settings::AiThinking;
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// How a local model is stored and which runtime runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    /// A single `.gguf` file for llama.cpp.
    Gguf,
    /// A folder with `config.json`, tokenizer and `.safetensors` weights for MLX.
    Mlx,
}

/// MLX models are folders; everything else is treated as a GGUF file.
pub fn format(path: &Path) -> ModelFormat {
    if path.is_dir() { ModelFormat::Mlx } else { ModelFormat::Gguf }
}

/// Checks that `path` is a usable model of its format.
pub fn validate(path: &Path) -> Result<()> {
    match format(path) {
        ModelFormat::Mlx => validate_mlx(path),
        ModelFormat::Gguf => validate_gguf(path),
    }
}

fn validate_gguf(path: &Path) -> Result<()> {
    let mut magic = [0; 4];
    File::open(path)
        .context("Open GGUF model")?
        .read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        bail!("Select a GGUF model file (not a Git LFS pointer or HTML page).");
    }
    Ok(())
}

/// An MLX model folder: a `config.json` naming the architecture and real
/// (not Git LFS pointer) `.safetensors` weights.
fn validate_mlx(dir: &Path) -> Result<()> {
    if !crate::mlx_runtime::supported() {
        bail!("MLX models need a Mac with Apple silicon.");
    }
    if !is_mlx_dir(dir) {
        bail!("Select an MLX model folder with config.json and .safetensors weights.");
    }
    Ok(())
}

fn is_mlx_dir(dir: &Path) -> bool {
    let config = fs::read(dir.join("config.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    if !config.is_some_and(|c| c.get("model_type").is_some()) {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else { return false };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.extension().is_some_and(|e| e == "safetensors")
            // A safetensors header starts with its little-endian length, never "vers".
            && File::open(&path)
                .and_then(|mut f| {
                    let mut head = [0; 8];
                    f.read_exact(&mut head).map(|_| head)
                })
                .is_ok_and(|head| &head[..4] != b"vers")
    })
}

/// Only Hugging Face resolve URLs are accepted; components cannot escape the cache.
pub fn download_url(repo: &str, file: &str) -> Result<String> {
    if !valid_repo(repo)
        || !file.split('/').all(valid_component)
        || !file.to_ascii_lowercase().ends_with(".gguf")
    {
        bail!("Enter a Hugging Face owner/repository and a GGUF filename (subfolders allowed).");
    }
    Ok(download_url_any(repo, file))
}

fn valid_component(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// `owner/repository` with safe components only.
pub fn valid_repo(repo: &str) -> bool {
    repo.split('/').count() == 2 && repo.split('/').all(valid_component)
}

/// A file in a Hugging Face repository listing.
#[derive(serde::Deserialize, Debug)]
struct RepoEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: u64,
    lfs: Option<LfsInfo>,
}

#[derive(serde::Deserialize, Debug)]
struct LfsInfo {
    /// SHA-256 of the file contents.
    oid: String,
}

/// The files an MLX model needs: config, tokenizer, chat template and weights.
/// Top-level only; docs, images and remote code are skipped.
fn mlx_files(entries: Vec<RepoEntry>) -> Result<Vec<RepoEntry>> {
    let files: Vec<RepoEntry> = entries
        .into_iter()
        .filter(|e| e.kind == "file" && !e.path.contains('/') && valid_component(&e.path))
        .filter(|e| {
            let name = e.path.to_ascii_lowercase();
            [".json", ".safetensors", ".txt", ".model", ".tiktoken", ".jinja"].iter().any(|ext| name.ends_with(ext))
                && !name.starts_with("readme")
        })
        .collect();
    if !files.iter().any(|f| f.path == "config.json") || !files.iter().any(|f| f.path.ends_with(".safetensors")) {
        bail!("Not an MLX model repository: it needs config.json and .safetensors weights. Use llama.cpp for GGUF models.");
    }
    if files.iter().any(|f| f.lfs.as_ref().is_some_and(|l| l.oid.len() != 64 || !l.oid.bytes().all(|b| b.is_ascii_hexdigit()))) {
        bail!("Unexpected Hugging Face file checksum");
    }
    Ok(files)
}

/// A model Rile recommends, downloadable in one click from Settings → AI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogModel {
    pub name: &'static str,
    pub blurb: &'static str,
    pub format: ModelFormat,
    /// Hugging Face `owner/repo`.
    pub repo: &'static str,
    /// The GGUF file in `repo`; empty for MLX, which downloads the whole repository.
    pub file: &'static str,
    /// Download size in bytes.
    pub size: u64,
    /// Memory (GB) a computer should have to run it comfortably.
    pub memory_gb: u64,
    /// The suggested pick for most computers.
    pub recommended: bool,
}

impl CatalogModel {
    /// Whether `model` is this entry, downloaded.
    pub fn is(&self, model: &InstalledModel) -> bool {
        model.format == self.format
            && model.repo.as_deref() == Some(self.repo)
            && (self.format == ModelFormat::Mlx || model.name() == self.file)
    }
}

/// Recommended chat models, small to large. Every architecture here runs on
/// the runtimes Rile installs; sizes are the repositories' Q4_K_M / 4-bit files.
pub const CATALOG: &[CatalogModel] = &[
    CatalogModel { name: "Qwen 3.5 2B", blurb: "Smallest and fastest. Good for search on any computer.", format: ModelFormat::Gguf, repo: "unsloth/Qwen3.5-2B-GGUF", file: "Qwen3.5-2B-Q4_K_M.gguf", size: 1_280_835_840, memory_gb: 4, recommended: false },
    CatalogModel { name: "Qwen 3.5 4B", blurb: "Quick, capable all-rounder.", format: ModelFormat::Gguf, repo: "unsloth/Qwen3.5-4B-GGUF", file: "Qwen3.5-4B-Q4_K_M.gguf", size: 2_740_937_888, memory_gb: 8, recommended: true },
    CatalogModel { name: "Gemma 4 E4B", blurb: "Google's efficient model. Strong at writing and summaries.", format: ModelFormat::Gguf, repo: "unsloth/gemma-4-E4B-it-GGUF", file: "gemma-4-E4B-it-Q4_K_M.gguf", size: 4_977_171_584, memory_gb: 8, recommended: false },
    CatalogModel { name: "Qwen 3.5 9B", blurb: "Better answers for longer documents; slower.", format: ModelFormat::Gguf, repo: "unsloth/Qwen3.5-9B-GGUF", file: "Qwen3.5-9B-Q4_K_M.gguf", size: 5_680_522_464, memory_gb: 16, recommended: false },
    CatalogModel { name: "Qwen 3.8 27B", blurb: "Highest quality. Needs a lot of memory.", format: ModelFormat::Gguf, repo: "unsloth/Qwen3.8-27B-GGUF", file: "Qwen3.8-27B-UD-Q4_K_M.gguf", size: 16_464_440_224, memory_gb: 32, recommended: false },
    CatalogModel { name: "Qwen 3.5 2B", blurb: "Smallest and fastest. Good for search on any Mac.", format: ModelFormat::Mlx, repo: "mlx-community/Qwen3.5-2B-MLX-4bit", file: "", size: 1_749_079_691, memory_gb: 4, recommended: false },
    CatalogModel { name: "Qwen 3.5 4B", blurb: "Quick, capable all-rounder.", format: ModelFormat::Mlx, repo: "mlx-community/Qwen3.5-4B-MLX-4bit", file: "", size: 3_061_129_077, memory_gb: 8, recommended: true },
    CatalogModel { name: "Gemma 4 E4B", blurb: "Google's efficient model. Strong at writing and summaries.", format: ModelFormat::Mlx, repo: "mlx-community/gemma-4-e4b-it-4bit", file: "", size: 5_179_239_349, memory_gb: 8, recommended: false },
    CatalogModel { name: "Qwen 3.5 9B", blurb: "Better answers for longer documents; slower.", format: ModelFormat::Mlx, repo: "mlx-community/Qwen3.5-9B-MLX-4bit", file: "", size: 5_977_071_067, memory_gb: 16, recommended: false },
    CatalogModel { name: "Qwen 3.8 27B", blurb: "Highest quality. Needs a lot of memory.", format: ModelFormat::Mlx, repo: "mlx-community/Qwen3.8-27B-4bit", file: "", size: 16_081_488_731, memory_gb: 32, recommended: false },
];

/// The [`CATALOG`] entries of `format`.
pub fn catalog(format: ModelFormat) -> impl Iterator<Item = &'static CatalogModel> {
    CATALOG.iter().filter(move |m| m.format == format)
}

/// Installed memory in bytes, when the platform reports it.
pub fn system_memory() -> Option<u64> {
    static MEMORY: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *MEMORY.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            let out = Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        }
        #[cfg(target_os = "linux")]
        {
            let info = fs::read_to_string("/proc/meminfo").ok()?;
            let kb: u64 = info.lines().find_map(|l| l.strip_prefix("MemTotal:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
            Some(kb * 1024)
        }
        #[cfg(windows)]
        {
            use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
            let mut status = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
            Some(status.ullTotalPhys)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
        None
    })
}

/// Whether a model needing `memory_gb` fits; true when memory is unknown.
/// Machines report a little under their marketed size (16 GB ≈ 17.2e9 bytes
/// on a Mac, less on Linux), so a 10% margin keeps a 16 GB model on a 16 GB machine.
pub fn fits(memory_gb: u64) -> bool {
    system_memory().is_none_or(|bytes| bytes as f64 >= memory_gb as f64 * (1u64 << 30) as f64 * 0.9)
}

/// Lists a repository's `main` revision.
fn repo_tree(repo: &str) -> Result<Vec<RepoEntry>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let json = agent
        .get(&format!("https://huggingface.co/api/models/{repo}/tree/main?recursive=true"))
        .call()
        .map_err(|error| match error {
            // Hugging Face answers 401 for repositories that don't exist.
            ureq::Error::StatusCode(401 | 403 | 404) => {
                anyhow::anyhow!("{repo} wasn't found on Hugging Face, or it is private or gated.")
            }
            error => anyhow::Error::new(error).context("List the Hugging Face repository"),
        })?
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()?;
    serde_json::from_str(&json).context("Read the repository listing")
}

/// The chat model GGUF files of a repository, with sizes: vision projectors
/// and the later shards of split models are left out. Blocking.
pub fn repo_ggufs(repo: &str) -> Result<Vec<(String, u64)>> {
    if !valid_repo(repo) {
        bail!("Enter a Hugging Face owner/repository, for example unsloth/Qwen3.5-4B-GGUF.");
    }
    let files = gguf_files(repo_tree(repo)?);
    if files.is_empty() {
        bail!("{repo} has no GGUF files. MLX models go in the MLX tab.");
    }
    Ok(files)
}

fn gguf_files(entries: Vec<RepoEntry>) -> Vec<(String, u64)> {
    let mut files: Vec<(String, u64)> = entries
        .into_iter()
        .filter(|e| e.kind == "file" && e.path.split('/').all(valid_component))
        .filter(|e| {
            let name = e.path.rsplit('/').next().unwrap_or_default().to_ascii_lowercase();
            // Split models need every shard, which a single download can't fetch.
            let split = is_later_shard(&name) || name.contains("-00001-of-");
            name.ends_with(".gguf") && !name.starts_with("mmproj") && !split
        })
        .map(|e| (e.path, e.size))
        .collect();
    files.sort_by_key(|(_, size)| *size);
    files
}

/// Downloads a whole MLX model repository (its `main` revision) into
/// `<models>/<owner>/<repo>`. The folder appears only once every file is
/// complete and large files match their SHA-256. `progress` gets bytes across
/// all files; returning false cancels and removes partial data.
pub fn download_mlx(repo: &str, mut progress: impl FnMut(u64, Option<u64>) -> bool) -> Result<PathBuf> {
    if !valid_repo(repo) {
        bail!("Enter a Hugging Face owner/repository, for example mlx-community/Qwen3-4B-4bit.");
    }
    let (owner, name) = repo.split_once('/').context("Repository")?;
    let parent = models_dir().context("Settings directory")?.join(owner);
    let destination = parent.join(name);
    if destination.exists() {
        if is_mlx_dir(&destination) {
            return Ok(destination);
        }
        bail!("{} already exists and isn't an MLX model.", destination.display());
    }
    let files = mlx_files(repo_tree(repo)?)?;
    let total: u64 = files.iter().map(|f| f.size).sum();
    fs::create_dir_all(&parent)?;
    // Hidden, so listings skip it; same filesystem for an atomic rename.
    let stage = parent.join(format!(".{name}.partial"));
    if stage.exists() {
        fs::remove_dir_all(&stage)?;
    }
    fs::create_dir(&stage)?;
    let result = (|| {
        let mut before = 0;
        for file in &files {
            let url = download_url_any(repo, &file.path);
            let path = stage.join(&file.path);
            fetch(&url, &path, |done, _| progress(before + done, (total > 0).then_some(total)))?;
            let size = fs::metadata(&path)?.len();
            if size != file.size {
                bail!("Incomplete download of {}", file.path);
            }
            if let Some(lfs) = &file.lfs
                && !crate::updater::sha256_file(&path)?.eq_ignore_ascii_case(&lfs.oid)
            {
                bail!("Checksum mismatch for {}; nothing was installed", file.path);
            }
            before += size;
        }
        if !is_mlx_dir(&stage) {
            bail!("The downloaded files aren't a usable MLX model.");
        }
        fs::rename(&stage, &destination).context("Publish model download")?;
        Ok(destination.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

/// Resolve URL for any (already validated) file of a repository.
fn download_url_any(repo: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{file}")
}

/// Downloads atomically. `progress` returning false cancels and removes partial data.
pub fn download(
    repo: &str,
    file: &str,
    progress: impl FnMut(u64, Option<u64>) -> bool,
) -> Result<PathBuf> {
    let url = download_url(repo, file)?;
    let dir = models_dir()
        .context("Settings directory")?
        .join(repo)
        .join(Path::new(file).parent().unwrap_or(Path::new("")));
    fs::create_dir_all(&dir)?;
    let path = dir.join(Path::new(file).file_name().context("Model filename")?);
    download_to(&url, &path, progress)?;
    Ok(path)
}

/// Where [`download`] stores models: `<data>/models/<owner>/<repo>/…`.
pub fn models_dir() -> Option<PathBuf> {
    Some(rile_core::settings::Settings::data_dir()?.join("models"))
}

/// A GGUF file or MLX folder found in [`models_dir`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledModel {
    pub path: PathBuf,
    /// The Hugging Face `owner/repo` it was downloaded from, when known.
    pub repo: Option<String>,
    pub size: u64,
    pub format: ModelFormat,
}

impl InstalledModel {
    pub fn name(&self) -> String {
        self.path.file_name().unwrap_or_default().to_string_lossy().into_owned()
    }
}

/// Downloaded models, sorted by repository and name. Blocking.
pub fn installed() -> Vec<InstalledModel> {
    models_dir().map(|dir| installed_in(&dir)).unwrap_or_default()
}

/// Chat models below `dir`: GGUF files and MLX folders. Skips partial
/// downloads (`*.part`, hidden folders), vision projectors (`mmproj-*`) and
/// the later shards of split models, which llama.cpp loads through the first one.
pub fn installed_in(dir: &Path) -> Vec<InstalledModel> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() && entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            } else if kind.is_dir() && is_mlx_dir(&path) {
                out.push(path);
            } else if kind.is_dir() && depth > 0 {
                walk(&path, depth - 1, out);
            } else if kind.is_file() {
                out.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, 6, &mut found);
    let mut models: Vec<InstalledModel> = found
        .into_iter()
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
            path.is_dir() || (name.ends_with(".gguf") && !name.starts_with("mmproj") && !is_later_shard(&name))
        })
        .filter(|path| validate(path).is_ok())
        .map(|path| {
            let format = format(&path);
            let mut parts = path.strip_prefix(dir).ok().map(|rel| rel.components()).into_iter().flatten();
            let repo = match (parts.next(), parts.next(), parts.next()) {
                (Some(owner), Some(repo), rest) if rest.is_some() || format == ModelFormat::Mlx => Some(format!(
                    "{}/{}",
                    owner.as_os_str().to_string_lossy(),
                    repo.as_os_str().to_string_lossy()
                )),
                _ => None,
            };
            let size = match format {
                ModelFormat::Gguf => fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                ModelFormat::Mlx => fs::read_dir(&path)
                    .map(|entries| entries.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
                    .unwrap_or(0),
            };
            InstalledModel { path, repo, size, format }
        })
        .collect();
    models.sort_by_cached_key(|m| (m.repo.clone().unwrap_or_default().to_lowercase(), m.name().to_lowercase()));
    models
}

/// Deletes a downloaded model from [`models_dir`]. Blocking.
pub fn remove(model: &Path) -> Result<()> {
    remove_in(&models_dir().context("Settings directory")?, model)
}

/// Deletes the GGUF file (with the other shards of a split model) or MLX
/// folder `model`, which must lie inside `dir`, then prunes folders left empty.
pub fn remove_in(dir: &Path, model: &Path) -> Result<()> {
    let dir = dunce::canonicalize(dir).context("Models folder")?;
    let model = dunce::canonicalize(model).context("Model not found")?;
    if model == dir || !model.starts_with(&dir) {
        bail!("{} isn't in Rile's models folder.", model.display());
    }
    if model.is_dir() {
        fs::remove_dir_all(&model).context("Delete model folder")?;
    } else {
        let parent = model.parent().context("Model folder")?;
        let name = model.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
        // `name-00001-of-00003.gguf` → every `name-0000N-of-00003.gguf`.
        let shards = name
            .strip_suffix(".gguf")
            .and_then(|stem| stem.rsplit_once("-of-"))
            .and_then(|(head, total)| Some((head.rsplit_once('-')?.0.to_string(), total.to_string())));
        fs::remove_file(&model).context("Delete model file")?;
        if let Some((base, total)) = shards {
            for entry in fs::read_dir(parent)?.flatten() {
                let other = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if other.starts_with(&format!("{base}-")) && other.ends_with(&format!("-of-{total}.gguf")) && is_later_shard(&other) {
                    fs::remove_file(entry.path()).context("Delete model shard")?;
                }
            }
        }
    }
    let mut folder = model.parent();
    while let Some(current) = folder.filter(|f| *f != dir && f.starts_with(&dir)) {
        if fs::remove_dir(current).is_err() {
            break;
        }
        folder = current.parent();
    }
    Ok(())
}

/// `name-00002-of-00003.gguf` and later: only the first shard is loadable.
fn is_later_shard(lower_name: &str) -> bool {
    let Some(stem) = lower_name.strip_suffix(".gguf") else { return false };
    let mut parts = stem.rsplitn(4, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(total), Some("of"), Some(index)) if total.len() == 5 && index.len() == 5 => {
            index.bytes().all(|b| b.is_ascii_digit()) && index != "00001"
        }
        _ => false,
    }
}

fn download_to(
    url: &str,
    path: &Path,
    progress: impl FnMut(u64, Option<u64>) -> bool,
) -> Result<()> {
    if path.exists() {
        validate_gguf(path)?;
        return Ok(());
    }
    let partial = path.with_extension("gguf.part");
    fetch(url, &partial, progress)?;
    let result = validate_gguf(&partial).and_then(|()| Ok(fs::rename(&partial, path)?));
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

/// Streams `url` into a new file at `path`, removing it on failure.
fn fetch(url: &str, path: &Path, mut progress: impl FnMut(u64, Option<u64>) -> bool) -> Result<()> {
    // create_new prevents two downloads writing the same partial file.
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .context("Create model download (remove a stale .part file if a previous run crashed)")?;
    let result = (|| {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_per_read(Some(Duration::from_secs(30)))
            .build()
            .into();
        let mut response = agent
            .get(url)
            .call()
            .context("Download from Hugging Face (use a public, ungated repository)")?;
        let expected = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let mut reader = response.body_mut().as_reader();
        let mut buffer = [0; 65536];
        let mut bytes = 0;
        loop {
            if !progress(bytes, expected.filter(|n| *n > 0)) {
                bail!("Download cancelled");
            }
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            bytes += count as u64;
        }
        if expected.is_some_and(|n| n != bytes) {
            bail!("Incomplete model download");
        }
        output.sync_all()?;
        Ok(())
    })();
    drop(output);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

type ChildHandle = std::sync::Arc<std::sync::Mutex<Child>>;
static SERVERS: std::sync::Mutex<Vec<std::sync::Weak<std::sync::Mutex<Child>>>> =
    std::sync::Mutex::new(Vec::new());

/// Stop active inference processes before the application exits.
pub fn shutdown() {
    *TARGET.lock().unwrap() = None;
    EPOCH.fetch_add(1, Ordering::SeqCst);
    for server in SERVERS
        .lock()
        .unwrap()
        .drain(..)
        .filter_map(|s| s.upgrade())
    {
        stop(&mut server.lock().unwrap());
    }
}

/// Kills a runtime started by [`start`]: on Unix the whole process group of its guard.
fn stop(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: plain syscall; the group id is the guard's pid, which we haven't reaped yet.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs the runtime with its arguments, stopping it when Rile goes away however
/// Rile exits (even a crash or SIGKILL, where no cleanup code runs). Rile holds
/// the only write end of the guard's stdin; at end of file the guard stops the
/// runtime. Otherwise the guard passes on the runtime's exit status.
#[cfg(unix)]
const GUARD: &str = r#"exec 3<&0 0</dev/null
"$@" 3<&- &
pid=$!
{ read _ <&3; kill $pid 2>/dev/null; } &
watcher=$!
exec 3<&-
wait $pid
status=$?
kill $watcher 2>/dev/null
exit $status"#;

fn runtime_command(executable: &str) -> Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("/bin/sh");
        // Its own process group, so stopping the guard stops the runtime too,
        // and a terminal's Ctrl-C reaches Rile, not the runtime.
        command.args(["-c", GUARD, "rile-runtime-guard", executable]).stdin(Stdio::piped()).process_group(0);
        command
    }
    #[cfg(not(unix))]
    {
        let mut command = Command::new(executable);
        command.stdin(Stdio::null());
        crate::no_window(&mut command);
        command
    }
}

/// Windows has no process-group trick: every runtime joins a job object that
/// kills its members when the job's last handle closes, which happens when
/// Rile exits however it exits (the handle is never closed explicitly).
#[cfg(windows)]
fn tie_to_rile(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    struct Job(HANDLE);
    // SAFETY: a job handle may be used from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}
    static JOB: std::sync::OnceLock<Option<Job>> = std::sync::OnceLock::new();
    let job = JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32).ok()?;
        Some(Job(job))
    });
    if let Some(job) = job {
        unsafe { AssignProcessToJobObject(job.0, HANDLE(child.as_raw_handle())).ok() };
    }
}

struct Server(ChildHandle);
impl Server {
    fn new(child: Child) -> Self {
        let child = std::sync::Arc::new(std::sync::Mutex::new(child));
        let mut servers = SERVERS.lock().unwrap();
        servers.retain(|s| s.strong_count() > 0);
        servers.push(std::sync::Arc::downgrade(&child));
        Self(child)
    }

    fn running(&self) -> bool {
        matches!(self.0.lock().unwrap().try_wait(), Ok(None))
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        stop(&mut self.0.lock().unwrap());
    }
}

/// Whether the selected model is held in memory by its runtime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum LoadState {
    #[default]
    Unloaded,
    Loading(PathBuf),
    Ready(PathBuf),
    Failed(PathBuf, String),
}

/// A runtime kept running with its model loaded, shared by all requests.
struct Resident {
    model: PathBuf,
    executable: String,
    base: String,
    server: Server,
}

/// Held while a model loads, so concurrent requests wait for the same load.
static RESIDENT: std::sync::Mutex<Option<Resident>> = std::sync::Mutex::new(None);
static STATE: std::sync::Mutex<LoadState> = std::sync::Mutex::new(LoadState::Unloaded);
/// Bumped by every [`keep_loaded`] change; a load in progress for an older
/// target gives up.
static EPOCH: AtomicU64 = AtomicU64::new(0);
static TARGET: std::sync::Mutex<Option<(PathBuf, String)>> = std::sync::Mutex::new(None);

pub fn load_state() -> LoadState {
    STATE.lock().unwrap().clone()
}

fn set_state(state: LoadState) {
    *STATE.lock().unwrap() = state;
}

/// Keeps `target` (model, runtime executable) loaded in the background,
/// unloading whatever was loaded before; `None` frees the memory. Returns at
/// once; repeating the current target does nothing.
pub fn keep_loaded(target: Option<(PathBuf, String)>) {
    {
        let mut current = TARGET.lock().unwrap();
        if *current == target {
            return;
        }
        current.clone_from(&target);
    }
    let epoch = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let current = move || EPOCH.load(Ordering::SeqCst) == epoch;
    std::thread::spawn(move || match target {
        Some((model, executable)) => {
            let _ = ensure(&model, &executable, current);
        }
        None => {
            if let Some(mut resident) = lock_resident(current) {
                *resident = None;
                set_state(LoadState::Unloaded);
            }
        }
    });
}

/// Waits for any load in progress; `None` once `keep` turns false.
fn lock_resident(mut keep: impl FnMut() -> bool) -> Option<std::sync::MutexGuard<'static, Option<Resident>>> {
    loop {
        if !keep() {
            return None;
        }
        match RESIDENT.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::Poisoned(error)) => return Some(error.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// The base URL of a runtime with `model` loaded, starting it (and stopping
/// the previous one) when needed. `None` if `keep` turned false first.
fn ensure(model: &Path, executable: &str, mut keep: impl FnMut() -> bool) -> Result<Option<String>> {
    let Some(mut resident) = lock_resident(&mut keep) else {
        return Ok(None);
    };
    if let Some(current) = resident.as_ref()
        && current.model == model
        && current.executable == executable
        && current.server.running()
    {
        return Ok(Some(current.base.clone()));
    }
    // Free the previous model's memory before loading the next one.
    *resident = None;
    set_state(LoadState::Loading(model.to_owned()));
    match start(model, executable, &mut keep) {
        Ok(Some(started)) => {
            let base = started.base.clone();
            *resident = Some(started);
            set_state(LoadState::Ready(model.to_owned()));
            Ok(Some(base))
        }
        Ok(None) => {
            set_state(LoadState::Unloaded);
            Ok(None)
        }
        Err(error) => {
            set_state(LoadState::Failed(model.to_owned(), format!("{error:#}")));
            Err(error)
        }
    }
}

/// Starts the runtime on a loopback port and waits until the model is in memory.
fn start(model: &Path, executable: &str, keep: &mut impl FnMut() -> bool) -> Result<Option<Resident>> {
    validate(model)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port().to_string();
    drop(listener);
    let mlx = format(model) == ModelFormat::Mlx;
    let runtime = if mlx { "mlx_lm.server" } else { "llama-server" };
    let mut command = runtime_command(executable);
    command.args(["--host", "127.0.0.1", "--port", &port]);
    if mlx {
        command.args(["--max-tokens", "2048", "--log-level", "WARNING"]);
    } else {
        command.args(["--ctx-size", "8192", "--parallel", "1"]);
    }
    let mut child = command
        .arg("--model")
        .arg(model)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!("Start {runtime}. Use Install Runtime in AI Settings, or set a custom executable path.")
        })?;
    #[cfg(windows)]
    tie_to_rile(&child);
    // Drain stderr so the runtime never blocks on a full pipe; keep the tail for diagnostics.
    let log = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<String>::new()));
    let stderr = child.stderr.take().map(|stderr| {
        let log = log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut log = log.lock().unwrap();
                if log.len() == 40 {
                    log.pop_front();
                }
                log.push_back(line);
            }
        })
    });
    let server = Server::new(child);
    let base = format!("http://127.0.0.1:{port}");
    let health: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(1)))
        .build()
        .into();
    let started = Instant::now();
    loop {
        if !keep() {
            return Ok(None);
        }
        if let Some(status) = server.0.lock().unwrap().try_wait()? {
            // The pipe closes on exit, so the reader finishes with the full tail.
            if let Some(reader) = stderr {
                let _ = reader.join();
            }
            // The guard's shell reports a missing executable as 127.
            if cfg!(unix) && status.code() == Some(127) {
                bail!("Start {runtime}: {executable} not found. Use Install Runtime in AI Settings, or set a custom executable path.");
            }
            let log = log.lock().unwrap();
            bail!(failure_message(runtime, &format!("exited ({status})"), log.iter().map(String::as_str)));
        }
        match health.get(&format!("{base}/health")).call() {
            Ok(_) => break,
            // mlx_lm.server keeps running but answers 503 once loading the model
            // failed (llama-server answers 503 while still loading).
            Err(ureq::Error::StatusCode(503)) if mlx => {
                std::thread::sleep(Duration::from_millis(200));
                let log = log.lock().unwrap();
                bail!(failure_message(runtime, "couldn't load the model", log.iter().map(String::as_str)));
            }
            Err(_) => {}
        }
        if started.elapsed() > Duration::from_secs(180) {
            bail!("Timed out loading the local model.");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if mlx {
        // mlx_lm.server loads the model lazily: a one-token request pulls it into memory.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .build()
            .into();
        let body = serde_json::json!({"messages": [{"role": "user", "content": "Hi"}], "max_tokens": 1});
        agent
            .post(&format!("{base}/v1/chat/completions"))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .context("Load the MLX model. Check model compatibility and available memory.")?;
    }
    Ok(Some(Resident { model: model.to_owned(), executable: executable.to_owned(), base, server }))
}

/// Explains a runtime that failed while loading, using its last error line when there is one.
fn failure_message<'a>(runtime: &str, what: &str, log: impl Iterator<Item = &'a str>) -> String {
    let log: Vec<&str> = log.collect();
    // llama.cpp prints the cause as an "error" line, then generic "failed to load" lines.
    let last = |word: &str| log.iter().rev().find(|line| line.to_ascii_lowercase().contains(word)).copied();
    let Some(error) = last("error").or_else(|| last("exception")).or_else(|| last("failed")) else {
        return format!("{runtime} {what}. Check model compatibility and available memory.");
    };
    // llama.cpp: "unknown model architecture: 'lfm3'"; mlx-lm: "Model type lfm3 not supported."
    let architecture = error.split("unknown model architecture: ").nth(1).or_else(|| {
        error.split("Model type ").nth(1)?.strip_suffix(" not supported.")
    });
    if let Some(architecture) = architecture {
        return format!(
            "This {runtime} version doesn't support the model architecture {}. Update the runtime in AI Settings, or use a model in another format.",
            architecture.trim()
        );
    }
    format!("{runtime} {what}: {}", error.trim())
}

/// Streams full response snapshots from the resident runtime, loading the
/// model first if it isn't in memory yet. The model stays loaded afterwards.
pub fn generate(
    model: &Path,
    executable: &str,
    prompt: &str,
    instructions: &str,
    thinking: AiThinking,
    mut emit: impl FnMut(String) -> bool,
) -> Result<()> {
    validate(model)?;
    let Some(base) = ensure(model, executable, || emit(String::new()))? else {
        return Ok(());
    };
    // Prompt processing happens before the first byte.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_recv_response(Some(Duration::from_secs(300)))
        .timeout_per_read(Some(Duration::from_secs(120)))
        .build()
        .into();
    let body = generation_request(format(model), prompt, instructions, thinking);
    let mut response = agent
        .post(&format!("{base}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .send(body.to_string())?;
    read_stream(BufReader::new(response.body_mut().as_reader()), emit)
}

/// Keep reasoning from consuming the answer's entire generation allowance.
/// This is a token budget, not a wall-clock timeout: speed varies by model.
fn generation_request(format: ModelFormat, prompt: &str, instructions: &str, thinking: AiThinking) -> serde_json::Value {
    let mut body = serde_json::json!({"messages": [
        {"role": "system", "content": instructions}, {"role": "user", "content": prompt}],
        "temperature": 0.2, "max_tokens": 2048, "stream": true});
    match format {
        // llama.cpp closes the thinking block and continues generating the answer.
        ModelFormat::Gguf => {
            body["reasoning_budget_tokens"] = thinking.token_budget().into();
            if thinking == AiThinking::None {
                body["chat_template_kwargs"] = serde_json::json!({"enable_thinking": false});
            }
        }
        // mlx-lm has no reasoning budget. Templates that support this switch
        // can skip thinking; other templates may ignore it.
        ModelFormat::Mlx => body["chat_template_kwargs"] = serde_json::json!({"enable_thinking": thinking != AiThinking::None}),
    }
    body
}

/// Snapshots of the reply so far. Reasoning that the server streams apart from
/// the content (llama-server's `reasoning_content`, mlx-lm's `reasoning`) is
/// put back in front as a `<think>…</think>` block, the way models write it inline.
fn read_stream(reader: impl BufRead, mut emit: impl FnMut(String) -> bool) -> Result<()> {
    let (mut reasoning, mut text) = (String::new(), String::new());
    let mut calls = std::collections::BTreeMap::<u64, (String, String)>::new();
    for line in reader.lines() {
        let line = line?;
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };
        if data == "[DONE]" {
            return Ok(());
        }
        let value: serde_json::Value = serde_json::from_str(data)?;
        if let Some(error) = value.get("error") {
            bail!("Local model: {error}");
        }
        let delta = &value["choices"][0]["delta"];
        let thought = ["reasoning_content", "reasoning"].iter().find_map(|k| delta[*k].as_str()).filter(|s| !s.is_empty());
        let content = delta["content"].as_str().filter(|s| !s.is_empty());
        // MLX extracts native model calls from the text. OpenAI-compatible
        // servers can send the function name and JSON arguments in fragments.
        let tool_calls = delta["tool_calls"].as_array();
        if let Some(tool_calls) = tool_calls {
            for call in tool_calls {
                let entry = calls.entry(call["index"].as_u64().unwrap_or(0)).or_default();
                entry.0.push_str(call["function"]["name"].as_str().unwrap_or_default());
                entry.1.push_str(call["function"]["arguments"].as_str().unwrap_or_default());
            }
        }
        if thought.is_none() && content.is_none() && tool_calls.is_none() {
            continue;
        }
        reasoning.push_str(thought.unwrap_or_default());
        text.push_str(content.unwrap_or_default());
        // Reuse the agent's text-form JSON protocol, but only once the first
        // call's arguments are complete; never execute a partial input.
        let mut answer = text.clone();
        if let Some((name, arguments)) = calls.first_key_value().map(|(_, call)| call)
            && !name.is_empty()
            && let Ok(arguments) = serde_json::from_str::<serde_json::Value>(arguments)
            && arguments.is_object()
        {
            answer.push_str(&serde_json::json!({"name": name, "arguments": arguments}).to_string());
        }
        let snapshot = match (reasoning.is_empty(), answer.is_empty()) {
            (true, _) => answer,
            (false, true) => format!("<think>{reasoning}"),
            (false, false) => format!("<think>{reasoning}</think>{answer}"),
        };
        if !emit(snapshot) {
            return Ok(());
        }
    }
    bail!("Local model response ended unexpectedly")
}

#[cfg(test)]
mod tests {
    #[test]
    fn generation_limits_reasoning_without_truncating_the_answer() {
        use super::{ModelFormat, generation_request};

        for format in [ModelFormat::Gguf, ModelFormat::Mlx] {
            let body = generation_request(format, "Question", "Instructions", AiThinking::default());
            assert_eq!(body["max_tokens"], 2048);
            assert_eq!(body["stream"], true);
            assert_eq!(body["messages"][0]["content"], "Instructions");
            assert_eq!(body["messages"][1]["content"], "Question");
            match format {
                ModelFormat::Gguf => {
                    assert_eq!(body["reasoning_budget_tokens"], 64);
                    assert!(body.get("chat_template_kwargs").is_none());
                }
                ModelFormat::Mlx => {
                    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
                    assert!(body.get("reasoning_budget_tokens").is_none());
                }
            }
        }
    }

    #[test]
    fn catalog_entries_are_downloadable_and_match_installs() {
        use super::*;
        for format in [ModelFormat::Gguf, ModelFormat::Mlx] {
            let entries: Vec<_> = catalog(format).collect();
            assert_eq!(entries.iter().filter(|m| m.recommended).count(), 1, "one recommendation per {format:?}");
            assert!(entries.windows(2).all(|w| w[0].size <= w[1].size), "small to large");
            for m in entries {
                assert!(valid_repo(m.repo));
                match format {
                    ModelFormat::Gguf => assert!(download_url(m.repo, m.file).is_ok()),
                    ModelFormat::Mlx => assert!(m.file.is_empty()),
                }
            }
        }
        let gguf = &CATALOG[0];
        let installed = |repo: &str, path: &str, format| InstalledModel { path: path.into(), repo: Some(repo.into()), size: 1, format };
        assert!(gguf.is(&installed(gguf.repo, &format!("/m/{}/{}", gguf.repo, gguf.file), ModelFormat::Gguf)));
        assert!(!gguf.is(&installed(gguf.repo, "/m/other.gguf", ModelFormat::Gguf)));
        let mlx = catalog(ModelFormat::Mlx).next().unwrap();
        assert!(mlx.is(&installed(mlx.repo, "/m/x", ModelFormat::Mlx)));
        assert!(!mlx.is(&installed("other/repo", "/m/x", ModelFormat::Mlx)));
    }

    #[test]
    fn repository_listing_keeps_single_file_chat_models() {
        use super::*;
        let entry = |path: &str, size| RepoEntry { kind: "file".into(), path: path.into(), size, lfs: None };
        let files = gguf_files(vec![
            entry("model-Q8_0.gguf", 8),
            entry("model-Q4_K_M.gguf", 4),
            entry("mmproj-F16.gguf", 1),
            entry("big/big-Q4-00001-of-00002.gguf", 5),
            entry("big/big-Q4-00002-of-00002.gguf", 5),
            entry("README.md", 1),
            RepoEntry { kind: "directory".into(), path: "x.gguf".into(), size: 0, lfs: None },
        ]);
        assert_eq!(files, vec![("model-Q4_K_M.gguf".to_string(), 4), ("model-Q8_0.gguf".to_string(), 8)]);
    }

    #[test]
    fn thinking_preferences_reach_each_runtime() {
        for thinking in [AiThinking::None, AiThinking::Low, AiThinking::Medium, AiThinking::High, AiThinking::Unlimited] {
            let gguf = generation_request(ModelFormat::Gguf, "", "", thinking);
            assert_eq!(gguf["reasoning_budget_tokens"], thinking.token_budget());
            assert_eq!(gguf["max_tokens"], 2048);
            if thinking == AiThinking::None {
                assert_eq!(gguf["chat_template_kwargs"]["enable_thinking"], false);
            }
            let mlx = generation_request(ModelFormat::Mlx, "", "", thinking);
            assert_eq!(mlx["chat_template_kwargs"]["enable_thinking"], thinking != AiThinking::None);
        }
    }

    #[test]
    fn failure_message_reports_runtime_cause() {
        let log = [
            "llama_model_load: error loading model: unknown model architecture: 'lfm3'",
            "srv    load_model: failed to load model, '/m.gguf'",
        ];
        let exited = "exited (exit status: 1)";
        let message = super::failure_message("llama-server", exited, log.into_iter());
        assert!(message.contains("architecture 'lfm3'"), "{message}");
        let mlx = ["ValueError: Model type lfm3 not supported.", "127.0.0.1 - - \"GET /health HTTP/1.1\" 503 -"];
        let message = super::failure_message("mlx_lm.server", "couldn't load the model", mlx.into_iter());
        assert!(message.contains("architecture lfm3."), "{message}");
        let message = super::failure_message("llama-server", exited, ["ggml_metal: failed to allocate buffer"].into_iter());
        assert!(message.ends_with("exited (exit status: 1): ggml_metal: failed to allocate buffer"), "{message}");
        let message = super::failure_message("llama-server", exited, std::iter::empty());
        assert!(message.contains("available memory"), "{message}");
    }

    use super::*;

    /// A stand-in runtime: answers health checks and streams "pid <its pid>".
    #[cfg(unix)]
    const FAKE_RUNTIME: &str = r#"#!/usr/bin/env python3
import http.server, json, os, sys
port = int(sys.argv[sys.argv.index("--port") + 1])
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b"ok")
    def do_POST(self):
        self.rfile.read(int(self.headers["Content-Length"]))
        delta = json.dumps({"choices": [{"delta": {"content": "pid %d" % os.getpid()}}]})
        self.send_response(200); self.end_headers()
        self.wfile.write(("data: %s\n\ndata: [DONE]\n" % delta).encode())
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
"#;

    #[cfg(unix)]
    fn wait_for(state: impl Fn(&LoadState) -> bool) -> LoadState {
        let start = Instant::now();
        loop {
            let current = load_state();
            if state(&current) || start.elapsed() > Duration::from_secs(20) {
                return current;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[cfg(unix)]
    #[test]
    fn keeps_one_model_resident_and_switches() {
        use std::os::unix::fs::PermissionsExt;
        if Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let runtime = dir.path().join("fake-server");
        fs::write(&runtime, FAKE_RUNTIME).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = runtime.to_string_lossy().into_owned();
        let (first, second) = (dir.path().join("a.gguf"), dir.path().join("b.gguf"));
        for path in [&first, &second] {
            fs::write(path, b"GGUFtest").unwrap();
        }
        let ask = |model: &Path| {
            let mut out = String::new();
            generate(model, &runtime, "hi", "", AiThinking::default(), |text| {
                out = text;
                true
            })
            .unwrap();
            out
        };

        // Loaded in the background, then reused by every request.
        keep_loaded(Some((first.clone(), runtime.clone())));
        assert_eq!(wait_for(|s| matches!(s, LoadState::Ready(_))), LoadState::Ready(first.clone()));
        let pid = ask(&first);
        assert!(pid.starts_with("pid "), "{pid}");
        assert_eq!(ask(&first), pid, "the resident runtime serves later requests");

        // Switching replaces the runtime; the previous process is stopped.
        keep_loaded(Some((second.clone(), runtime.clone())));
        assert_eq!(wait_for(|s| *s == LoadState::Ready(second.clone())), LoadState::Ready(second.clone()));
        assert_ne!(ask(&second), pid);
        let old = pid.trim_start_matches("pid ");
        assert!(!Command::new("kill").args(["-0", old]).stderr(Stdio::null()).status().unwrap().success(), "previous model unloaded");

        keep_loaded(None);
        assert_eq!(wait_for(|s| *s == LoadState::Unloaded), LoadState::Unloaded);
        assert!(RESIDENT.lock().unwrap().is_none());
    }
    #[cfg(unix)]
    #[test]
    fn guard_stops_runtime_when_rile_goes_away() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let mut guard = runtime_command("/bin/sh")
            .args(["-c", "echo $$ > \"$0\"; exec sleep 60"])
            .arg(&pid_file)
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let pid = loop {
            if let Some(pid) = fs::read_to_string(&pid_file).ok().filter(|p| p.ends_with('\n')) {
                break pid.trim().to_owned();
            }
            assert!(started.elapsed() < Duration::from_secs(5), "runtime started");
            std::thread::sleep(Duration::from_millis(20));
        };
        // Rile exiting closes its end of the guard's stdin.
        drop(guard.stdin.take());
        assert!(guard.wait().unwrap().code().is_some_and(|c| c != 0));
        let alive = || Command::new("kill").args(["-0", &pid]).stderr(Stdio::null()).status().unwrap().success();
        let started = Instant::now();
        while alive() {
            assert!(started.elapsed() < Duration::from_secs(5), "runtime stopped with Rile");
            std::thread::sleep(Duration::from_millis(20));
        }

        // A runtime exiting on its own reports its status through the guard.
        // Holds stdin open like Rile does: `wait` and `status` would close it first.
        let exit = |command: &mut Command| {
            let mut child = command.stderr(Stdio::null()).spawn().unwrap();
            let _stdin = child.stdin.take();
            child.wait().unwrap().code()
        };
        assert_eq!(exit(runtime_command("/bin/sh").args(["-c", "exit 3"])), Some(3));
        assert_eq!(exit(&mut runtime_command("/nonexistent/llama-server")), Some(127));
    }
    #[test]
    fn snapshots_errors_and_cancellation() {
        let data = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\ndata: [DONE]\n";
        let mut snapshots = Vec::new();
        read_stream(&data[..], |text| {
            snapshots.push(text);
            true
        })
        .unwrap();
        assert_eq!(snapshots, ["Hello", "Hello world"]);
        read_stream(&data[..], |_| false).unwrap();
        assert!(read_stream(&b"data: {\"error\":\"bad model\"}\n"[..], |_| true).is_err());
        let reasoning = b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Hmm\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n";
        let mut snapshots = Vec::new();
        read_stream(&reasoning[..], |text| {
            snapshots.push(text);
            true
        })
        .unwrap();
        assert_eq!(snapshots, ["<think>Hmm", "<think>Hmm</think>Hi"]);
        assert!(read_stream(&b"data: {}\n"[..], |_| true).is_err());
    }

    #[test]
    fn native_tool_calls_preserve_reasoning_and_wait_for_complete_arguments() {
        let chunks = [
            serde_json::json!({"reasoning": "Search for invoices."}),
            serde_json::json!({"content": "\n"}),
            serde_json::json!({"tool_calls": [{"index": 0, "function": {"name": "search_", "arguments": ""}}]}),
            serde_json::json!({"tool_calls": [{"index": 0, "function": {"name": "files", "arguments": "{\"input\":\"outgoing"}}]}),
            serde_json::json!({"tool_calls": [{"index": 0, "function": {"arguments": " invoices arickinda\"}"}}]}),
        ];
        let mut data: String = chunks.iter().map(|delta| format!("data: {}\n\n", serde_json::json!({"choices": [{"delta": delta}]}))).collect();
        data.push_str("data: [DONE]\n");
        let mut snapshots = Vec::new();
        read_stream(data.as_bytes(), |text| { snapshots.push(text); true }).unwrap();
        assert_eq!(snapshots.len(), 5);
        assert!(!snapshots[3].contains("search_"));
        let final_reply = snapshots.last().unwrap();
        let (thought, action) = final_reply.strip_prefix("<think>").unwrap().split_once("</think>").unwrap();
        assert_eq!(thought, "Search for invoices.");
        assert_eq!(serde_json::from_str::<serde_json::Value>(action).unwrap(),
            serde_json::json!({"name": "search_files", "arguments": {"input": "outgoing invoices arickinda"}}));

        // MLX sends a complete call in one delta, even without tools in the request.
        let data = format!("data: {}\n\ndata: [DONE]\n", serde_json::json!({"choices": [{"delta": {"tool_calls": [
            {"index": 0, "function": {"name": "list_folder", "arguments": "{\"input\":\"~/Documents\"}"}}
        ]}}]}));
        let mut snapshots = Vec::new();
        read_stream(data.as_bytes(), |text| { snapshots.push(text); false }).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&snapshots[0]).unwrap(),
            serde_json::json!({"name": "list_folder", "arguments": {"input": "~/Documents"}}));
    }

    fn mock_download(
        body: &'static [u8],
        claimed_length: Option<usize>,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let length_header = claimed_length
                .map(|n| format!("Content-Length: {n}\r\n"))
                .unwrap_or_default();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\n{length_header}Connection: close\r\n\r\n"
            );
            let _ = stream.write_all(body);
        });
        (url, thread)
    }

    #[test]
    fn downloads_publish_only_valid_completed_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        for (body, length, cancel) in [
            (b"GGUFdata".as_slice(), 8, false),
            (b"GGUFdata".as_slice(), 8, true),
            (b"<html>error</html>".as_slice(), 18, false),
            (b"GGUFdata".as_slice(), 100, false),
        ] {
            let (url, thread) = mock_download(body, Some(length));
            let mut progress = Vec::new();
            let result = download_to(&url, &path, |done, total| {
                progress.push((done, total));
                !cancel
            });
            thread.join().unwrap();
            assert_eq!(
                result.is_ok(),
                body == b"GGUFdata" && length == 8 && !cancel
            );
            assert!(!path.with_extension("gguf.part").exists());
            if result.is_ok() {
                assert_eq!(progress.first(), Some(&(0, Some(length as u64))));
                assert_eq!(progress.last(), Some(&(length as u64, Some(length as u64))));
                assert_eq!(fs::read(&path).unwrap(), body);
                fs::remove_file(&path).unwrap();
            } else {
                assert!(!path.exists());
            }
        }
    }
    #[test]
    fn unknown_download_total_stays_indeterminate() {
        let dir = tempfile::tempdir().unwrap();
        let (url, thread) = mock_download(b"GGUFdata", None);
        let mut progress = Vec::new();
        download_to(&url, &dir.path().join("model.gguf"), |done, total| {
            progress.push((done, total));
            true
        })
        .unwrap();
        thread.join().unwrap();
        assert_eq!(progress.first(), Some(&(0, None)));
        assert_eq!(progress.last(), Some(&(8, None)));
    }

    #[test]
    fn download_paths_are_safe() {
        assert_eq!(
            download_url("org/model", "sub/model.gguf").unwrap(),
            "https://huggingface.co/org/model/resolve/main/sub/model.gguf"
        );
        for (repo, file) in [
            ("../model", "a.gguf"),
            ("org/model", "../a.gguf"),
            ("org/model", "/a.gguf"),
            ("org/model", "a.bin"),
            ("org/model", "a.gguf?x"),
        ] {
            assert!(download_url(repo, file).is_err());
        }
    }
    #[test]
    fn lists_downloaded_models() {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, bytes: &[u8]| {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        };
        write("org/chat/chat-Q4_K_M.gguf", b"GGUFchat");
        write("org/chat/mmproj-chat-f16.gguf", b"GGUFproj");
        write("org/big/big-00001-of-00002.gguf", b"GGUFone");
        write("org/big/big-00002-of-00002.gguf", b"GGUFtwo");
        write("org/chat/half.gguf.part", b"GGUF");
        write("org/chat/pointer.gguf", b"version https://git-lfs");
        write("loose.gguf", b"GGUFloose");
        let models = installed_in(dir.path());
        let names: Vec<_> = models.iter().map(|m| (m.repo.as_deref(), m.name())).collect();
        assert_eq!(
            names,
            [
                (None, "loose.gguf".to_string()),
                (Some("org/big"), "big-00001-of-00002.gguf".to_string()),
                (Some("org/chat"), "chat-Q4_K_M.gguf".to_string()),
            ]
        );
        assert_eq!(models[2].size, 8);
        assert!(installed_in(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn removes_downloaded_models() {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str| {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"GGUFdata").unwrap();
            path
        };
        let chat = write("org/chat/chat.gguf");
        let other = write("org/chat/other.gguf");
        let big = write("org/big/big-00001-of-00002.gguf");
        write("org/big/big-00002-of-00002.gguf");
        write_mlx(&dir.path().join("mlx/model"));
        let outside = tempfile::tempdir().unwrap();
        let stray = outside.path().join("x.gguf");
        fs::write(&stray, b"GGUF").unwrap();
        assert!(remove_in(dir.path(), &stray).is_err());
        assert!(remove_in(dir.path(), dir.path()).is_err());
        assert!(stray.exists());
        // A file goes alone; its folder stays while other models are in it.
        remove_in(dir.path(), &chat).unwrap();
        assert!(!chat.exists() && other.exists());
        // Split models take all their shards, and emptied folders go too.
        remove_in(dir.path(), &big).unwrap();
        assert!(!dir.path().join("org/big").exists());
        remove_in(dir.path(), &dir.path().join("mlx/model")).unwrap();
        assert!(!dir.path().join("mlx").exists());
        remove_in(dir.path(), &other).unwrap();
        assert!(!dir.path().join("org").exists() && dir.path().exists());
    }

    fn write_mlx(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("config.json"), br#"{"model_type": "qwen3"}"#).unwrap();
        fs::write(dir.join("tokenizer.json"), b"{}").unwrap();
        fs::write(dir.join("model.safetensors"), [8, 0, 0, 0, 0, 0, 0, 0, b'{', b'}']).unwrap();
    }

    #[test]
    fn lists_and_validates_mlx_folders() {
        let dir = tempfile::tempdir().unwrap();
        write_mlx(&dir.path().join("mlx-community/Qwen3-4B-4bit"));
        write_mlx(&dir.path().join("mlx-community/.Other.partial"));
        // Weights that are still Git LFS pointers aren't a model.
        let pointer = dir.path().join("org/pointer");
        write_mlx(&pointer);
        fs::write(pointer.join("model.safetensors"), b"version https://git-lfs").unwrap();
        fs::write(dir.path().join("org/chat.gguf"), b"GGUFchat").unwrap();
        let models = installed_in(dir.path());
        if !crate::mlx_runtime::supported() {
            assert_eq!(models.len(), 1);
            return;
        }
        let names: Vec<_> = models.iter().map(|m| (m.repo.as_deref(), m.name(), m.format)).collect();
        assert_eq!(
            names,
            [
                (None, "chat.gguf".to_string(), ModelFormat::Gguf),
                (Some("mlx-community/Qwen3-4B-4bit"), "Qwen3-4B-4bit".to_string(), ModelFormat::Mlx),
            ]
        );
        assert_eq!(models[1].size, 23 + 2 + 10);
        validate(&models[1].path).unwrap();
        assert!(validate(&pointer).is_err());
        assert!(validate(dir.path()).is_err());
    }

    #[test]
    fn mlx_downloads_take_only_model_files() {
        let entry = |path: &str, lfs: Option<&str>| RepoEntry {
            kind: "file".into(),
            path: path.into(),
            size: 1,
            lfs: lfs.map(|oid| LfsInfo { oid: oid.into() }),
        };
        let hash = "a".repeat(64);
        let files = mlx_files(vec![
            entry("config.json", None),
            entry("model.safetensors", Some(&hash)),
            entry("tokenizer.json", None),
            entry("chat_template.jinja", None),
            entry("README.md", None),
            entry(".gitattributes", None),
            entry("modeling_custom.py", None),
            entry("sub/extra.safetensors", None),
        ])
        .unwrap();
        let names: Vec<_> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(names, ["config.json", "model.safetensors", "tokenizer.json", "chat_template.jinja"]);
        // GGUF repositories and bad checksums are refused.
        assert!(mlx_files(vec![entry("config.json", None), entry("model.gguf", None)]).is_err());
        assert!(mlx_files(vec![entry("config.json", None), entry("model.safetensors", Some("x"))]).is_err());
        assert!(valid_repo("mlx-community/Qwen3-4B-4bit"));
        assert!(!valid_repo("../x") && !valid_repo("a/b/c") && !valid_repo("a"));
    }

    #[test]
    #[ignore = "downloads a small MLX model and runs mlx_lm.server from PATH"]
    fn mlx_generation_smoke() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: ignored test, run on its own.
        unsafe { std::env::set_var("RILE_CONFIG_DIR", dir.path()) };
        let model = download_mlx("mlx-community/Qwen3-0.6B-4bit", |_, _| true).unwrap();
        let mut last = String::new();
        generate(&model, "mlx_lm.server", "Say hello.", "Answer briefly.", AiThinking::default(), |text| {
            if !text.is_empty() {
                last = text;
            }
            true
        })
        .unwrap();
        assert!(!last.is_empty());
    }

    #[test]
    fn rejects_non_models() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        assert!(validate(temp.path()).is_err());
        fs::write(temp.path(), b"version https://git-lfs.github.com/spec/v1").unwrap();
        assert!(validate(temp.path()).is_err());
        fs::write(temp.path(), b"GGUFtest").unwrap();
        validate(temp.path()).unwrap();
    }
}
