//! Creating and extracting archives: zip, 7z, tar (plain, gzip, brotli,
//! zstd) and single-file gzip/brotli/zstd streams.
//!
//! Both directions are synchronous, honour a cancel flag and report
//! [`ArchiveProgress`] through a callback; run them on a background thread.
//!
//! Symlinks are stored as links in every multi-entry format (tar natively,
//! zip and 7z with the Info-ZIP/p7zip unix-mode convention) and are never
//! followed while walking. Single-stream formats (gz/br/zst) compress the
//! file a symlink points to.
//!
//! Extraction is hardened against hostile archives: entries with `..`,
//! absolute or drive-prefixed paths abort the extraction, nothing is ever
//! written through a symlink, and symlinks whose target is absolute or
//! escapes the extraction root are skipped. Everything is unpacked into a
//! hidden staging folder in `dest_dir` first, so a failure or cancellation
//! leaves nothing behind.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::ops::RangeInclusive;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use rile_core::ops::unique_destination;

const CANCELLED: &str = "Cancelled";
/// Minimum interval between intra-file progress callbacks.
const EMIT_INTERVAL: Duration = Duration::from_millis(30);
/// Largest solid block written to 7z archives (same limit the crate uses).
const SEVENZ_MAX_BLOCK: u64 = 4 * 1024 * 1024 * 1024;
const MAX_LINK_TARGET: u64 = 4096;

#[cfg(unix)]
const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFREG: u32 = 0o100_000;
const S_IFLNK: u32 = 0o120_000;
/// 7z attribute flag: the high 16 bits hold a unix `st_mode`.
const SEVENZ_UNIX_EXTENSION: u32 = 0x8000;
const SEVENZ_ATTR_DIRECTORY: u32 = 0x10;

// ---------------------------------------------------------------------------
// Formats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Tar,
    TarGz,
    TarBr,
    TarZst,
    /// Single-file gzip stream.
    Gz,
    /// Single-file brotli stream.
    Br,
    /// Single-file zstd stream.
    Zst,
}

impl ArchiveFormat {
    pub const ALL: [ArchiveFormat; 9] = [
        Self::Zip,
        Self::SevenZ,
        Self::Tar,
        Self::TarGz,
        Self::TarBr,
        Self::TarZst,
        Self::Gz,
        Self::Br,
        Self::Zst,
    ];

    /// File extension without the leading dot (may contain a dot itself).
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZ => "7z",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::TarBr => "tar.br",
            Self::TarZst => "tar.zst",
            Self::Gz => "gz",
            Self::Br => "br",
            Self::Zst => "zst",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Zip => "ZIP",
            Self::SevenZ => "7-Zip",
            Self::Tar => "Tar",
            Self::TarGz => "Tar + Gzip",
            Self::TarBr => "Tar + Brotli",
            Self::TarZst => "Tar + Zstandard",
            Self::Gz => "Gzip",
            Self::Br => "Brotli",
            Self::Zst => "Zstandard",
        }
    }

    /// Valid compression levels; `None` when the format doesn't compress.
    pub fn level_range(self) -> Option<RangeInclusive<u32>> {
        match self {
            Self::Zip | Self::SevenZ => Some(0..=9),
            Self::Tar => None,
            Self::TarGz | Self::Gz => Some(1..=9),
            Self::TarBr | Self::Br => Some(0..=11),
            Self::TarZst | Self::Zst => Some(1..=22),
        }
    }

    pub fn default_level(self) -> u32 {
        match self {
            Self::Zip | Self::TarGz | Self::Gz => 6,
            Self::SevenZ => 5,
            Self::Tar => 0,
            Self::TarBr | Self::Br => 9,
            Self::TarZst | Self::Zst => 3,
        }
    }

    /// Whether the format can hold folders / several items. The single-stream
    /// formats (gz, br, zst) only compress exactly one regular file.
    pub fn supports_multiple(self) -> bool {
        !matches!(self, Self::Gz | Self::Br | Self::Zst)
    }

    fn is_tar(self) -> bool {
        matches!(self, Self::Tar | Self::TarGz | Self::TarBr | Self::TarZst)
    }

    fn clamp_level(self, level: u32) -> u32 {
        match self.level_range() {
            Some(r) => level.clamp(*r.start(), *r.end()),
            None => 0,
        }
    }
}

/// Suffixes recognised by [`detect`], longest first.
const SUFFIXES: &[(&str, ArchiveFormat)] = &[
    (".tar.gz", ArchiveFormat::TarGz),
    (".tar.br", ArchiveFormat::TarBr),
    (".tar.zst", ArchiveFormat::TarZst),
    (".tgz", ArchiveFormat::TarGz),
    (".tzst", ArchiveFormat::TarZst),
    (".tar", ArchiveFormat::Tar),
    (".zip", ArchiveFormat::Zip),
    (".zst", ArchiveFormat::Zst),
    (".7z", ArchiveFormat::SevenZ),
    (".gz", ArchiveFormat::Gz),
    (".br", ArchiveFormat::Br),
];

fn split_suffix(path: &Path) -> Option<(String, ArchiveFormat)> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let lower = name.to_ascii_lowercase();
    SUFFIXES.iter().find_map(|(suffix, format)| {
        lower.ends_with(suffix).then(|| (name[..name.len() - suffix.len()].to_string(), *format))
    })
}

/// Detects the archive format from the file name's extension.
pub fn detect(path: &Path) -> Option<ArchiveFormat> {
    split_suffix(path).map(|(_, f)| f)
}

/// Default archive file name for `inputs`: `foo` -> `foo.zip`,
/// `notes.txt` -> `notes.txt.gz`, several inputs -> `Archive.zip`.
pub fn suggested_name(inputs: &[PathBuf], format: ArchiveFormat) -> String {
    let base = match inputs {
        [single] => single
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Archive".into()),
        _ => "Archive".into(),
    };
    format!("{base}.{}", format.extension())
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveProgress {
    /// Path of the entry being processed, relative to the archive root.
    pub current: PathBuf,
    pub file_bytes_done: u64,
    pub file_bytes_total: u64,
    /// Overall bytes. For compression and zip/7z extraction these are
    /// uncompressed bytes; for tar-based and single-stream extraction they
    /// are compressed bytes read out of the archive file's size.
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub items_done: usize,
    /// 0 when unknown (tar-based extraction).
    pub items_total: usize,
}

impl ArchiveProgress {
    pub fn fraction(&self) -> f32 {
        if self.bytes_total == 0 {
            return if self.items_total == 0 { 1.0 } else { self.items_done as f32 / self.items_total as f32 };
        }
        (self.bytes_done as f64 / self.bytes_total as f64).min(1.0) as f32
    }

    pub fn file_fraction(&self) -> f32 {
        if self.file_bytes_total == 0 {
            return 1.0;
        }
        (self.file_bytes_done as f64 / self.file_bytes_total as f64).min(1.0) as f32
    }
}

struct Tracker<'a> {
    p: ArchiveProgress,
    cancel: &'a AtomicBool,
    cb: &'a mut dyn FnMut(&ArchiveProgress),
    /// When set, overall `bytes_done` mirrors this counter (compressed bytes read).
    external: Option<Rc<Cell<u64>>>,
    last: Instant,
}

impl<'a> Tracker<'a> {
    fn new(cancel: &'a AtomicBool, cb: &'a mut dyn FnMut(&ArchiveProgress)) -> Self {
        Self { p: ArchiveProgress::default(), cancel, cb, external: None, last: Instant::now() }
    }

    fn emit(&mut self, force: bool) {
        if let Some(c) = &self.external {
            self.p.bytes_done = c.get();
        }
        if force || self.last.elapsed() >= EMIT_INTERVAL {
            self.last = Instant::now();
            (self.cb)(&self.p);
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn check(&self) -> Result<()> {
        if self.cancelled() {
            bail!(CANCELLED);
        }
        Ok(())
    }

    fn begin(&mut self, current: &Path, total: u64) {
        self.p.current = current.to_path_buf();
        self.p.file_bytes_done = 0;
        self.p.file_bytes_total = total;
        self.emit(false);
    }

    fn advance(&mut self, n: u64) -> io::Result<()> {
        if self.cancelled() {
            return Err(io::Error::other(CANCELLED));
        }
        self.p.file_bytes_done += n;
        if self.external.is_none() {
            self.p.bytes_done += n;
        }
        self.emit(false);
        Ok(())
    }

    fn finish_item(&mut self) {
        self.p.items_done += 1;
        self.emit(false);
    }
}

/// Reader that feeds the bytes it yields into a [`Tracker`] and aborts when
/// the operation is cancelled.
struct Tracked<'t, 'a, R> {
    inner: R,
    t: &'t RefCell<Tracker<'a>>,
}

impl<R: Read> Read for Tracked<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.t.borrow_mut().advance(n as u64)?;
        Ok(n)
    }
}

/// Counts bytes read from the underlying (compressed) archive file.
struct Counting<R> {
    inner: R,
    count: Rc<Cell<u64>>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.set(self.count.get() + n as u64);
        Ok(n)
    }
}

/// Turns any error into "Cancelled" when the flag is set, so callers can
/// tell cancellation apart from failures.
fn map_cancel<T>(res: Result<T>, cancel: &AtomicBool) -> Result<T> {
    match res {
        Err(_) if cancel.load(Ordering::Relaxed) => Err(anyhow!(CANCELLED)),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Compression
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum ItemKind {
    Dir,
    File,
    Symlink(PathBuf),
}

#[derive(Debug)]
struct Item {
    src: PathBuf,
    /// Path inside the archive (relative, starts with the input's name).
    rel: PathBuf,
    kind: ItemKind,
    size: u64,
    /// Permission bits (no file-type bits).
    mode: u32,
    mtime: Option<SystemTime>,
}

impl Item {
    fn name(&self) -> String {
        entry_name(&self.rel)
    }

    fn unix_mode(&self) -> u32 {
        let ty = match self.kind {
            ItemKind::Dir => S_IFDIR,
            ItemKind::File => S_IFREG,
            ItemKind::Symlink(_) => S_IFLNK,
        };
        ty | (self.mode & 0o7777)
    }
}

fn entry_name(rel: &Path) -> String {
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

fn perm_bits(meta: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        if meta.is_dir() { 0o755 } else if meta.permissions().readonly() { 0o444 } else { 0o644 }
    }
}

fn collect(inputs: &[PathBuf], cancel: &AtomicBool) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    for input in inputs {
        let name = input.file_name().with_context(|| format!("\"{}\" has no file name", input.display()))?;
        walk(input, PathBuf::from(name), &mut out, cancel)?;
    }
    Ok(out)
}

fn walk(path: &Path, rel: PathBuf, out: &mut Vec<Item>, cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!(CANCELLED);
    }
    let meta = fs::symlink_metadata(path).with_context(|| format!("Can't read \"{}\"", path.display()))?;
    let ft = meta.file_type();
    let mut item = Item {
        src: path.to_path_buf(),
        rel,
        kind: ItemKind::File,
        size: 0,
        mode: perm_bits(&meta),
        mtime: meta.modified().ok(),
    };
    if ft.is_symlink() {
        item.kind = ItemKind::Symlink(fs::read_link(path)?);
        out.push(item);
    } else if ft.is_dir() {
        item.kind = ItemKind::Dir;
        let rel = item.rel.clone();
        out.push(item);
        let mut children = fs::read_dir(path)
            .with_context(|| format!("Can't read folder \"{}\"", path.display()))?
            .collect::<io::Result<Vec<_>>>()?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            walk(&child.path(), rel.join(child.file_name()), out, cancel)?;
        }
    } else if ft.is_file() {
        item.size = meta.len();
        out.push(item);
    }
    // Sockets, FIFOs and devices are skipped.
    Ok(())
}

/// Compresses `inputs` into `dest`. Each input is stored under its own name
/// (paths are relative to the input's parent); folders are recursed.
///
/// The archive is written to a hidden temp file next to `dest` and renamed
/// into place on success (replacing any existing `dest`); on error or
/// cancellation the partial file is deleted. `level` is clamped into
/// [`ArchiveFormat::level_range`]. Single-stream formats require exactly one
/// regular file (symlinks to files are followed).
pub fn compress(
    inputs: &[PathBuf],
    dest: &Path,
    format: ArchiveFormat,
    level: u32,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&ArchiveProgress),
) -> Result<()> {
    if inputs.is_empty() {
        bail!("Nothing to compress");
    }
    let level = format.clamp_level(level);

    let items = if format.supports_multiple() {
        collect(inputs, cancel)?
    } else {
        let [input] = inputs else {
            bail!("{} can only compress a single file; use a Tar format for multiple items", format.label());
        };
        let meta = fs::metadata(input).with_context(|| format!("Can't read \"{}\"", input.display()))?;
        if !meta.is_file() {
            bail!("{} can only compress a single file; use a Tar format for folders", format.label());
        }
        let name = input.file_name().context("input has no file name")?;
        vec![Item {
            src: input.clone(),
            rel: PathBuf::from(name),
            kind: ItemKind::File,
            size: meta.len(),
            mode: perm_bits(&meta),
            mtime: meta.modified().ok(),
        }]
    };

    let dir = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let tmp = tempfile::Builder::new()
        .prefix(".rile-archive-")
        .suffix(".part")
        .tempfile_in(dir)
        .with_context(|| format!("Can't create a file in \"{}\"", dir.display()))?;
    let file = tmp.as_file().try_clone()?;

    let mut cb = |p: &ArchiveProgress| on_progress(p);
    let mut tracker = Tracker::new(cancel, &mut cb);
    tracker.p.items_total = items.len();
    tracker.p.bytes_total = items.iter().map(|i| i.size).sum();
    tracker.emit(true);
    let tracker = RefCell::new(tracker);

    let res = match format {
        ArchiveFormat::Zip => write_zip(file, &items, level, &tracker),
        ArchiveFormat::SevenZ => write_7z(file, &items, level, &tracker),
        _ => {
            let enc = Encoder::new(file, format, level, &items[0]);
            enc.and_then(|mut enc| {
                if format.is_tar() {
                    write_tar(&mut enc, &items, &tracker)?;
                } else {
                    write_single(&mut enc, &items[0], &tracker)?;
                }
                enc.finish()?;
                Ok(())
            })
        }
    };
    map_cancel(res, cancel)?;
    tracker.borrow().check()?;

    tmp.persist(dest).map_err(|e| e.error).with_context(|| format!("Can't write \"{}\"", dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(0o644));
    }
    tracker.into_inner().emit(true);
    Ok(())
}

fn copy_file_tracked(item: &Item, out: &mut dyn Write, t: &RefCell<Tracker<'_>>) -> Result<()> {
    t.borrow_mut().begin(&item.rel, item.size);
    let f = File::open(&item.src).with_context(|| format!("Can't open \"{}\"", item.src.display()))?;
    let mut r = Tracked { inner: BufReader::with_capacity(256 * 1024, f), t };
    io::copy(&mut r, out)?;
    Ok(())
}

fn write_zip(file: File, items: &[Item], level: u32, t: &RefCell<Tracker<'_>>) -> Result<()> {
    use zip::CompressionMethod;
    use zip::write::{SimpleFileOptions, ZipWriter};

    let mut zw = ZipWriter::new(BufWriter::new(file));
    for item in items {
        t.borrow().check()?;
        let mut opts = SimpleFileOptions::default()
            .last_modified_time(zip_time(item.mtime))
            .unix_permissions(item.mode)
            .large_file(item.size >= u32::MAX as u64);
        opts = if level == 0 {
            opts.compression_method(CompressionMethod::Stored)
        } else {
            opts.compression_method(CompressionMethod::Deflated).compression_level(Some(level as i64))
        };
        let name = item.name();
        match &item.kind {
            ItemKind::Dir => zw.add_directory(format!("{name}/"), opts)?,
            ItemKind::Symlink(target) => zw.add_symlink(name, target.to_string_lossy(), opts)?,
            ItemKind::File => {
                zw.start_file(name, opts)?;
                copy_file_tracked(item, &mut zw, t)?;
            }
        }
        t.borrow_mut().finish_item();
    }
    zw.finish()?.flush()?;
    Ok(())
}

/// 7z source that opens its file on first read, so solid blocks don't hold
/// every file open at once.
struct LazySource<'t, 'a, 'i> {
    item: &'i Item,
    file: Option<BufReader<File>>,
    done: bool,
    t: &'t RefCell<Tracker<'a>>,
}

impl Read for LazySource<'_, '_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.file.is_none() {
            if self.t.borrow().cancelled() {
                return Err(io::Error::other(CANCELLED));
            }
            self.file = Some(BufReader::with_capacity(256 * 1024, File::open(&self.item.src)?));
            self.t.borrow_mut().begin(&self.item.rel, self.item.size);
        }
        let n = self.file.as_mut().unwrap().read(buf)?;
        if n == 0 {
            self.done = true;
            self.file = None;
            self.t.borrow_mut().finish_item();
        } else {
            self.t.borrow_mut().advance(n as u64)?;
        }
        Ok(n)
    }
}

fn write_7z(file: File, items: &[Item], level: u32, t: &RefCell<Tracker<'_>>) -> Result<()> {
    use sevenz_rust2::encoder_options::Lzma2Options;
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, NtTime, SourceReader};

    fn entry_for(item: &Item) -> ArchiveEntry {
        let name = item.name();
        let mut e = match item.kind {
            ItemKind::Dir => ArchiveEntry::new_directory(&name),
            _ => ArchiveEntry::new_file(&name),
        };
        // sevenz-rust2 0.20 writes the kAnti bitset inverted: every
        // empty-stream entry (dirs, empty files) whose flag is *false* gets
        // marked as an anti-item (a deletion marker), which 7-Zip and the
        // crate's own reader then honour. With the flag set on every entry
        // the writer omits the property entirely, which is what we want.
        e.is_anti_item = true;
        e.has_windows_attributes = true;
        e.windows_attributes = SEVENZ_UNIX_EXTENSION | (item.unix_mode() << 16);
        if matches!(item.kind, ItemKind::Dir) {
            e.windows_attributes |= SEVENZ_ATTR_DIRECTORY;
        }
        if let Some(Ok(nt)) = item.mtime.map(NtTime::try_from) {
            e.last_modified_date = nt;
            e.has_last_modified_date = true;
        }
        e
    }

    let mut w = ArchiveWriter::new(BufWriter::new(file))?;
    w.set_content_methods(vec![Lzma2Options::from_level(level).into()]);

    let mut entries = Vec::new();
    let mut sources: Vec<SourceReader<LazySource>> = Vec::new();
    let mut block_size = 0u64;
    for item in items {
        t.borrow().check()?;
        match &item.kind {
            ItemKind::Dir => {
                w.push_archive_entry::<&[u8]>(entry_for(item), None)?;
                t.borrow_mut().finish_item();
            }
            ItemKind::Symlink(target) => {
                let bytes = target.to_string_lossy().into_owned().into_bytes();
                w.push_archive_entry(entry_for(item), Some(bytes.as_slice()))?;
                t.borrow_mut().finish_item();
            }
            ItemKind::File => {
                if !entries.is_empty() && block_size + item.size >= SEVENZ_MAX_BLOCK {
                    w.push_archive_entries(std::mem::take(&mut entries), std::mem::take(&mut sources))?;
                    block_size = 0;
                }
                block_size += item.size;
                entries.push(entry_for(item));
                sources.push(LazySource { item, file: None, done: false, t }.into());
            }
        }
    }
    if !entries.is_empty() {
        w.push_archive_entries(entries, sources)?;
    }
    t.borrow().check()?;
    w.finish()?.flush()?;
    Ok(())
}

fn write_tar(w: &mut dyn Write, items: &[Item], t: &RefCell<Tracker<'_>>) -> Result<()> {
    use tar::{EntryType, Header};

    let mut b = tar::Builder::new(w);
    for item in items {
        t.borrow().check()?;
        let mut h = Header::new_gnu();
        h.set_mode(item.mode);
        h.set_mtime(item.mtime.and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()));
        match &item.kind {
            ItemKind::Dir => {
                h.set_entry_type(EntryType::Directory);
                h.set_size(0);
                b.append_data(&mut h, &item.rel, io::empty())?;
            }
            ItemKind::Symlink(target) => {
                h.set_entry_type(EntryType::Symlink);
                h.set_size(0);
                b.append_link(&mut h, &item.rel, target)?;
            }
            ItemKind::File => {
                h.set_entry_type(EntryType::Regular);
                h.set_size(item.size);
                t.borrow_mut().begin(&item.rel, item.size);
                let f = File::open(&item.src).with_context(|| format!("Can't open \"{}\"", item.src.display()))?;
                // `take` keeps the header size honest if the file grows meanwhile.
                let r = Tracked { inner: BufReader::with_capacity(256 * 1024, f).take(item.size), t };
                b.append_data(&mut h, &item.rel, r)?;
            }
        }
        t.borrow_mut().finish_item();
    }
    b.into_inner()?;
    Ok(())
}

fn write_single(w: &mut dyn Write, item: &Item, t: &RefCell<Tracker<'_>>) -> Result<()> {
    copy_file_tracked(item, w, t)?;
    t.borrow_mut().finish_item();
    Ok(())
}

/// Wraps a writer and remembers the first error, for encoders (brotli) whose
/// finishing step swallows I/O errors.
struct ErrTrack<W> {
    inner: W,
    err: Option<io::Error>,
}

impl<W: Write> Write for ErrTrack<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf).inspect_err(|e| {
            self.err.get_or_insert_with(|| io::Error::new(e.kind(), e.to_string()));
        })
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().inspect_err(|e| {
            self.err.get_or_insert_with(|| io::Error::new(e.kind(), e.to_string()));
        })
    }
}

/// Output stream for tar-based and single-stream formats.
enum Encoder {
    Plain(BufWriter<File>),
    Gz(flate2::write::GzEncoder<BufWriter<File>>),
    Br(Box<brotli::CompressorWriter<ErrTrack<BufWriter<File>>>>),
    Zst(zstd::stream::write::Encoder<'static, BufWriter<File>>),
}

impl Encoder {
    fn new(file: File, format: ArchiveFormat, level: u32, single: &Item) -> Result<Self> {
        use ArchiveFormat::*;
        let w = BufWriter::with_capacity(256 * 1024, file);
        Ok(match format {
            Tar => Self::Plain(w),
            TarGz => Self::Gz(flate2::write::GzEncoder::new(w, flate2::Compression::new(level))),
            Gz => {
                let mtime = single.mtime.and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                let name = single.rel.to_string_lossy().into_owned();
                Self::Gz(
                    flate2::GzBuilder::new()
                        .filename(name.into_bytes())
                        .mtime(mtime.min(u32::MAX as u64) as u32)
                        .write(w, flate2::Compression::new(level)),
                )
            }
            TarBr | Br => Self::Br(Box::new(brotli::CompressorWriter::new(
                ErrTrack { inner: w, err: None },
                64 * 1024,
                level,
                22,
            ))),
            TarZst | Zst => {
                let mut enc = zstd::stream::write::Encoder::new(w, level as i32)?;
                enc.include_checksum(true)?;
                Self::Zst(enc)
            }
            Zip | SevenZ => unreachable!("not a stream format"),
        })
    }

    fn finish(self) -> io::Result<()> {
        match self {
            Self::Plain(mut w) => w.flush(),
            Self::Gz(e) => e.finish()?.flush(),
            Self::Br(e) => {
                let mut track = e.into_inner();
                if let Some(err) = track.err.take() {
                    return Err(err);
                }
                track.inner.flush()
            }
            Self::Zst(e) => e.finish()?.flush(),
        }
    }
}

impl Write for Encoder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(buf),
            Self::Gz(w) => w.write(buf),
            Self::Br(w) => w.write(buf),
            Self::Zst(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Gz(w) => w.flush(),
            Self::Br(w) => w.flush(),
            Self::Zst(w) => w.flush(),
        }
    }
}

/// Converts to an MS-DOS timestamp in local time (UTC off macOS).
fn zip_time(t: Option<SystemTime>) -> zip::DateTime {
    let Some(secs) = t.and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64) else {
        return zip::DateTime::default();
    };
    let (y, mo, d, h, mi, s) = civil_time(secs);
    let Ok(y) = u16::try_from(y) else { return zip::DateTime::default() };
    zip::DateTime::from_date_and_time(y, mo, d, h, mi, s.min(58)).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn civil_time(secs: i64) -> (i64, u8, u8, u8, u8, u8) {
    // SAFETY: localtime_r only writes into the provided tm.
    unsafe {
        let t: libc::time_t = secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return utc_civil(secs);
        }
        (
            tm.tm_year as i64 + 1900,
            (tm.tm_mon + 1) as u8,
            tm.tm_mday as u8,
            tm.tm_hour as u8,
            tm.tm_min as u8,
            tm.tm_sec as u8,
        )
    }
}

#[cfg(not(target_os = "macos"))]
fn civil_time(secs: i64) -> (i64, u8, u8, u8, u8, u8) {
    utc_civil(secs)
}

/// Howard Hinnant's days-to-civil algorithm.
fn utc_civil(secs: i64) -> (i64, u8, u8, u8, u8, u8) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8)
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// Extracts `archive` into `dest_dir` and returns the created root.
///
/// An archive with a single top-level entry is extracted directly into
/// `dest_dir` (renamed `"name copy"` etc. if taken); otherwise a new folder
/// named after the archive (without its extension) is created. Unsafe entry
/// paths abort the extraction; on error or cancellation nothing is left in
/// `dest_dir`.
/// Total uncompressed size of the archive's contents, or `None` if `path`
/// isn't a supported archive. Zip and 7z read their directory; stream
/// formats are decoded (without writing anything), honouring `cancel`.
pub fn uncompressed_size(path: &Path, cancel: &AtomicBool) -> Result<Option<u64>> {
    let Some((_, format)) = split_suffix(path) else { return Ok(None) };
    let open = || File::open(path).with_context(|| format!("Can't open \"{}\"", path.display()));
    let cancelled = || anyhow!("cancelled");
    match format {
        ArchiveFormat::Zip => {
            let mut za = zip::ZipArchive::new(BufReader::new(open()?))?;
            let mut total = 0u64;
            for i in 0..za.len() {
                let e = za.by_index_raw(i)?;
                if !e.is_dir() {
                    total += e.size();
                }
            }
            Ok(Some(total))
        }
        ArchiveFormat::SevenZ => {
            let reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
                .with_context(|| format!("Can't open \"{}\"", path.display()))?;
            Ok(Some(reader.archive().files.iter().filter(|f| !f.is_directory).map(|f| f.size).sum()))
        }
        _ => {
            let raw = BufReader::with_capacity(256 * 1024, open()?);
            let mut decoder: Box<dyn Read> = match format {
                ArchiveFormat::Tar => Box::new(raw),
                ArchiveFormat::TarGz | ArchiveFormat::Gz => Box::new(flate2::read::MultiGzDecoder::new(raw)),
                ArchiveFormat::TarBr | ArchiveFormat::Br => Box::new(brotli::Decompressor::new(raw, 64 * 1024)),
                ArchiveFormat::TarZst | ArchiveFormat::Zst => Box::new(zstd::stream::read::Decoder::new(raw)?),
                ArchiveFormat::Zip | ArchiveFormat::SevenZ => unreachable!(),
            };
            if format.is_tar() {
                let mut total = 0u64;
                for entry in tar::Archive::new(decoder).entries()? {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(cancelled());
                    }
                    let entry = entry?;
                    if entry.header().entry_type().is_file() {
                        total += entry.size();
                    }
                }
                Ok(Some(total))
            } else {
                let mut buf = vec![0u8; 256 * 1024];
                let mut total = 0u64;
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(cancelled());
                    }
                    match decoder.read(&mut buf)? {
                        0 => break Ok(Some(total)),
                        n => total += n as u64,
                    }
                }
            }
        }
    }
}

pub fn extract(
    archive: &Path,
    dest_dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(&ArchiveProgress),
) -> Result<PathBuf> {
    let (stem, format) = split_suffix(archive).with_context(|| format!("\"{}\" isn't a supported archive", archive.display()))?;
    let stem = if stem.is_empty() { "Archive".to_string() } else { stem };

    let staging = tempfile::Builder::new()
        .prefix(".rile-extract-")
        .tempdir_in(dest_dir)
        .with_context(|| format!("Can't create a folder in \"{}\"", dest_dir.display()))?;

    let mut cb = |p: &ArchiveProgress| on_progress(p);
    let tracker = RefCell::new(Tracker::new(cancel, &mut cb));
    let mut sink = Sink { root: staging.path().to_path_buf(), dirs: HashSet::new(), dir_modes: Vec::new() };

    let res = match format {
        ArchiveFormat::Zip => extract_zip(archive, &mut sink, &tracker),
        ArchiveFormat::SevenZ => extract_7z(archive, &mut sink, &tracker),
        _ => extract_stream(archive, format, &stem, &mut sink, &tracker),
    };
    map_cancel(res, cancel)?;
    tracker.borrow().check()?;
    sink.apply_dir_modes();

    let mut top = fs::read_dir(staging.path())?.collect::<io::Result<Vec<_>>>()?;
    let created = if top.len() == 1 {
        let entry = top.pop().unwrap();
        let target = unique_destination(dest_dir, &entry.file_name().to_string_lossy());
        fs::rename(entry.path(), &target).with_context(|| format!("Can't create \"{}\"", target.display()))?;
        target
    } else {
        let target = unique_destination(dest_dir, &stem);
        let path = staging.keep();
        if let Err(e) = fs::rename(&path, &target) {
            let _ = fs::remove_dir_all(&path);
            return Err(e).with_context(|| format!("Can't create \"{}\"", target.display()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target, fs::Permissions::from_mode(0o755));
        }
        target
    };
    let mut t = tracker.into_inner();
    t.external = None;
    t.p.bytes_done = t.p.bytes_total;
    t.p.items_total = t.p.items_total.max(t.p.items_done);
    t.emit(true);
    Ok(created)
}

/// Turns an archive entry path into a safe relative path. `Ok(None)` for
/// entries that name the root itself (`./`); errors on `..`, absolute or
/// drive-prefixed paths.
fn safe_relative(raw: &Path) -> Result<Option<PathBuf>> {
    let mut out = PathBuf::new();
    for c in raw.components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("Refusing to extract unsafe path \"{}\"", raw.display())
            }
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// Zip and 7z names: treat backslashes as separators and reject drive letters.
fn safe_relative_str(name: &str) -> Result<Option<PathBuf>> {
    let normalized = name.replace('\\', "/");
    let first = normalized.split('/').next().unwrap_or("");
    if first.len() == 2 && first.ends_with(':') {
        bail!("Refusing to extract unsafe path \"{name}\"");
    }
    safe_relative(Path::new(&normalized))
}

fn is_macos_junk(rel: &Path) -> bool {
    rel.components().next().is_some_and(|c| c.as_os_str() == "__MACOSX")
}

/// Whether a symlink at `link` (relative to the root) pointing at `target`
/// stays inside the root, judged lexically.
fn link_stays_inside(link: &Path, target: &Path) -> bool {
    let mut depth = link.components().count().saturating_sub(1) as isize;
    for c in target.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// Writes entries below `root`, never through symlinks.
struct Sink {
    root: PathBuf,
    /// Directories (relative) verified to be real directories.
    dirs: HashSet<PathBuf>,
    dir_modes: Vec<(PathBuf, u32)>,
}

impl Sink {
    /// Makes sure every ancestor of `rel` is a real directory (creating it if
    /// needed) and returns the absolute path for `rel`.
    fn ensure_dir(&mut self, rel: &Path) -> Result<PathBuf> {
        let mut cur = PathBuf::new();
        for c in rel.components() {
            cur.push(c);
            if self.dirs.contains(&cur) {
                continue;
            }
            let abs = self.root.join(&cur);
            match fs::symlink_metadata(&abs) {
                Ok(m) if m.is_dir() => {}
                Ok(_) => bail!("Refusing to write through \"{}\" in archive", cur.display()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&abs)?,
                Err(e) => return Err(e.into()),
            }
            self.dirs.insert(cur.clone());
        }
        Ok(self.root.join(rel))
    }

    /// Prepares the location for a non-directory entry: parents exist, and
    /// any previous non-directory entry with the same name is removed.
    fn prepare_leaf(&mut self, rel: &Path) -> Result<PathBuf> {
        if let Some(parent) = rel.parent() {
            self.ensure_dir(parent)?;
        }
        let abs = self.root.join(rel);
        match fs::symlink_metadata(&abs) {
            Ok(m) if m.is_dir() => bail!("\"{}\" appears both as a folder and a file in the archive", rel.display()),
            Ok(_) => fs::remove_file(&abs)?,
            Err(_) => {}
        }
        Ok(abs)
    }

    fn dir(&mut self, rel: &Path, mode: Option<u32>, t: &RefCell<Tracker<'_>>) -> Result<()> {
        self.ensure_dir(rel)?;
        if let Some(mode) = mode {
            self.dir_modes.push((rel.to_path_buf(), mode));
        }
        let mut t = t.borrow_mut();
        t.begin(rel, 0);
        t.finish_item();
        Ok(())
    }

    fn file(
        &mut self,
        rel: &Path,
        reader: &mut dyn Read,
        size: u64,
        mode: Option<u32>,
        mtime: Option<SystemTime>,
        t: &RefCell<Tracker<'_>>,
    ) -> Result<()> {
        t.borrow().check()?;
        let abs = self.prepare_leaf(rel)?;
        t.borrow_mut().begin(rel, size);
        // create_new: never follows a symlink planted at the final component.
        let f = fs::OpenOptions::new().write(true).create_new(true).open(&abs)?;
        let mut out = BufWriter::with_capacity(256 * 1024, f);
        io::copy(&mut Tracked { inner: reader, t }, &mut out)?;
        let f = out.into_inner().map_err(|e| e.into_error())?;
        if let Some(mtime) = mtime {
            let _ = f.set_modified(mtime);
        }
        drop(f);
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&abs, fs::Permissions::from_mode(mode & 0o777))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        t.borrow_mut().finish_item();
        Ok(())
    }

    /// Creates a symlink if its target stays inside the root; skips it otherwise.
    fn symlink(&mut self, rel: &Path, target: &Path, t: &RefCell<Tracker<'_>>) -> Result<()> {
        if link_stays_inside(rel, target) {
            let abs = self.prepare_leaf(rel)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &abs)?;
            #[cfg(not(unix))]
            let _ = abs;
        }
        let mut t = t.borrow_mut();
        t.begin(rel, 0);
        t.finish_item();
        Ok(())
    }

    /// Tar hard link to an earlier entry.
    fn hard_link(&mut self, rel: &Path, target_rel: &Path, t: &RefCell<Tracker<'_>>) -> Result<()> {
        // Verify the target's ancestors are real dirs so we never resolve through a link.
        if let Some(parent) = target_rel.parent() {
            self.ensure_dir(parent)?;
        }
        let src = self.root.join(target_rel);
        let meta = fs::symlink_metadata(&src)
            .with_context(|| format!("Hard link target \"{}\" missing in archive", target_rel.display()))?;
        if !meta.is_file() {
            bail!("Hard link target \"{}\" isn't a file", target_rel.display());
        }
        let abs = self.prepare_leaf(rel)?;
        if fs::hard_link(&src, &abs).is_err() {
            fs::copy(&src, &abs)?;
        }
        let mut t = t.borrow_mut();
        t.begin(rel, 0);
        t.finish_item();
        Ok(())
    }

    fn apply_dir_modes(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Deepest first, and always keep the owner able to manage the tree.
            self.dir_modes.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
            for (rel, mode) in &self.dir_modes {
                let _ = fs::set_permissions(self.root.join(rel), fs::Permissions::from_mode((mode & 0o777) | 0o700));
            }
        }
    }
}

fn read_link_target(r: &mut dyn Read) -> Result<PathBuf> {
    let mut buf = Vec::new();
    r.take(MAX_LINK_TARGET).read_to_end(&mut buf)?;
    Ok(PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
}

#[cfg(unix)]
fn mode_is_symlink(mode: u32) -> bool {
    mode & S_IFMT == S_IFLNK
}
#[cfg(not(unix))]
fn mode_is_symlink(mode: u32) -> bool {
    mode & 0o170_000 == S_IFLNK
}

fn extract_zip(archive: &Path, sink: &mut Sink, t: &RefCell<Tracker<'_>>) -> Result<()> {
    let f = File::open(archive).with_context(|| format!("Can't open \"{}\"", archive.display()))?;
    let mut za = zip::ZipArchive::new(BufReader::new(f))?;

    // Validate every name and compute totals from the central directory first.
    let mut total = 0u64;
    for i in 0..za.len() {
        let e = za.by_index_raw(i)?;
        safe_relative_str(e.name())?;
        if !e.is_dir() {
            total += e.size();
        }
    }
    {
        let mut t = t.borrow_mut();
        t.p.bytes_total = total;
        t.p.items_total = za.len();
        t.emit(true);
    }

    for i in 0..za.len() {
        t.borrow().check()?;
        let mut e = za.by_index(i)?;
        let rel = match safe_relative_str(e.name())? {
            Some(rel) if !is_macos_junk(&rel) => rel,
            _ => {
                t.borrow_mut().finish_item();
                continue;
            }
        };
        let mode = e.unix_mode();
        if e.is_dir() {
            sink.dir(&rel, mode, t)?;
        } else if e.is_symlink() || mode.is_some_and(mode_is_symlink) {
            let target = read_link_target(&mut e)?;
            sink.symlink(&rel, &target, t)?;
        } else {
            let size = e.size();
            sink.file(&rel, &mut e, size, mode, None, t)?;
        }
    }
    Ok(())
}

fn extract_7z(archive: &Path, sink: &mut Sink, t: &RefCell<Tracker<'_>>) -> Result<()> {
    use sevenz_rust2::{ArchiveReader, Password};

    let mut reader = ArchiveReader::open(archive, Password::empty())
        .with_context(|| format!("Can't open \"{}\"", archive.display()))?;
    {
        let files = &reader.archive().files;
        for f in files {
            safe_relative_str(f.name())?;
        }
        let mut t = t.borrow_mut();
        t.p.bytes_total = files.iter().filter(|f| !f.is_directory).map(|f| f.size).sum();
        t.p.items_total = files.len();
        t.emit(true);
    }

    let mut failure: Option<anyhow::Error> = None;
    let res = reader.for_each_entries(|entry, r| {
        let mut step = || -> Result<()> {
            t.borrow().check()?;
            let rel = match safe_relative_str(entry.name())? {
                // Anti-items are ignored on purpose: real ones only occur in
                // update archives, and buggy writers (sevenz-rust2 0.20) flag
                // ordinary dirs/empty files with it.
                Some(rel) if !is_macos_junk(&rel) => rel,
                _ => {
                    io::copy(r, &mut io::sink())?;
                    t.borrow_mut().finish_item();
                    return Ok(());
                }
            };
            let attrs = entry.windows_attributes;
            let mode = (entry.has_windows_attributes && attrs & SEVENZ_UNIX_EXTENSION != 0).then_some(attrs >> 16);
            let mtime = entry.has_last_modified_date.then(|| SystemTime::from(entry.last_modified_date));
            if entry.is_directory {
                sink.dir(&rel, mode, t)
            } else if mode.is_some_and(mode_is_symlink) {
                let target = read_link_target(r)?;
                io::copy(r, &mut io::sink())?;
                sink.symlink(&rel, &target, t)
            } else {
                sink.file(&rel, r, entry.size, mode, mtime, t)
            }
        };
        match step() {
            Ok(()) => Ok(true),
            Err(e) => {
                failure = Some(e);
                Err(sevenz_rust2::Error::Other("aborted".into()))
            }
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    res?;
    Ok(())
}

/// Tar-based and single-stream formats: progress is compressed bytes read.
fn extract_stream(
    archive: &Path,
    format: ArchiveFormat,
    stem: &str,
    sink: &mut Sink,
    t: &RefCell<Tracker<'_>>,
) -> Result<()> {
    let f = File::open(archive).with_context(|| format!("Can't open \"{}\"", archive.display()))?;
    let count = Rc::new(Cell::new(0u64));
    {
        let mut t = t.borrow_mut();
        t.p.bytes_total = f.metadata()?.len();
        t.p.items_total = if format.is_tar() { 0 } else { 1 };
        t.external = Some(count.clone());
        t.emit(true);
    }
    let raw = Counting { inner: BufReader::with_capacity(256 * 1024, f), count };
    let mut decoder: Box<dyn Read> = match format {
        ArchiveFormat::Tar => Box::new(raw),
        ArchiveFormat::TarGz | ArchiveFormat::Gz => Box::new(flate2::read::MultiGzDecoder::new(raw)),
        ArchiveFormat::TarBr | ArchiveFormat::Br => Box::new(brotli::Decompressor::new(raw, 64 * 1024)),
        ArchiveFormat::TarZst | ArchiveFormat::Zst => Box::new(zstd::stream::read::Decoder::new(raw)?),
        ArchiveFormat::Zip | ArchiveFormat::SevenZ => unreachable!(),
    };

    if !format.is_tar() {
        let rel = PathBuf::from(stem);
        return sink.file(&rel, &mut decoder, 0, None, None, t);
    }

    let mut ar = tar::Archive::new(decoder);
    for entry in ar.entries()? {
        t.borrow().check()?;
        let mut entry = entry?;
        let Some(rel) = safe_relative(&entry.path()?)? else { continue };
        if is_macos_junk(&rel) {
            continue;
        }
        let header = entry.header();
        let mode = header.mode().ok();
        let mtime = header.mtime().ok().map(|s| UNIX_EPOCH + Duration::from_secs(s));
        use tar::EntryType as E;
        match header.entry_type() {
            E::Directory => sink.dir(&rel, mode, t)?,
            E::Regular | E::Continuous | E::GNUSparse => {
                let size = entry.size();
                sink.file(&rel, &mut entry, size, mode, mtime, t)?;
            }
            E::Symlink => {
                let target = entry.link_name()?.context("symlink without a target")?.into_owned();
                sink.symlink(&rel, &target, t)?;
            }
            E::Link => {
                let target = entry.link_name()?.context("hard link without a target")?;
                let target_rel = safe_relative(&target)?.context("hard link to the archive root")?;
                sink.hard_link(&rel, &target_rel, t)?;
            }
            // Devices, FIFOs and metadata-only records are skipped.
            _ => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn noop(_: &ArchiveProgress) {}

    /// project/
    ///   a.txt, run.sh (0755), empty_dir/, sub/b.bin (compressible-ish 300 KB),
    ///   sub/deeper/c.txt, link -> a.txt (unix)
    fn fixture(root: &Path) -> PathBuf {
        let p = root.join("project");
        fs::create_dir_all(p.join("sub/deeper")).unwrap();
        fs::create_dir_all(p.join("empty_dir")).unwrap();
        fs::write(p.join("a.txt"), "hello archive\n").unwrap();
        fs::write(p.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        let mut big = Vec::with_capacity(300_000);
        let mut x: u32 = 12345;
        for i in 0..300_000u32 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            big.push(if i % 3 == 0 { (x >> 16) as u8 } else { b'a' + (i % 7) as u8 });
        }
        fs::write(p.join("sub/b.bin"), &big).unwrap();
        fs::write(p.join("sub/deeper/c.txt"), "deep\n").unwrap();
        fs::write(p.join("sub/empty.txt"), "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(p.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("a.txt", p.join("link")).unwrap();
        }
        p
    }

    fn assert_same_tree(a: &Path, b: &Path) {
        let ma = fs::symlink_metadata(a).unwrap();
        let mb = fs::symlink_metadata(b).unwrap_or_else(|_| panic!("missing {}", b.display()));
        if ma.file_type().is_symlink() {
            assert!(mb.file_type().is_symlink(), "{} should be a symlink", b.display());
            assert_eq!(fs::read_link(a).unwrap(), fs::read_link(b).unwrap());
        } else if ma.is_dir() {
            assert!(mb.is_dir(), "{} should be a dir", b.display());
            let mut names_a: Vec<_> = fs::read_dir(a).unwrap().map(|e| e.unwrap().file_name()).collect();
            let mut names_b: Vec<_> = fs::read_dir(b).unwrap().map(|e| e.unwrap().file_name()).collect();
            names_a.sort();
            names_b.sort();
            assert_eq!(names_a, names_b, "children of {}", b.display());
            for n in names_a {
                assert_same_tree(&a.join(&n), &b.join(&n));
            }
        } else {
            assert!(mb.is_file());
            assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap(), "contents of {}", b.display());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(ma.permissions().mode() & 0o111, mb.permissions().mode() & 0o111, "exec bit of {}", b.display());
            }
        }
    }

    fn only_entries(dir: &Path) -> Vec<String> {
        let mut v: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    fn roundtrip_folder(format: ArchiveFormat, level: u32) {
        let tmp = TempDir::new().unwrap();
        let src = fixture(tmp.path());
        let out_dir = tmp.path().join("out");
        fs::create_dir(&out_dir).unwrap();
        let archive = tmp.path().join(suggested_name(std::slice::from_ref(&src), format));
        let mut last = ArchiveProgress::default();
        compress(std::slice::from_ref(&src), &archive, format, level, &AtomicBool::new(false), |p| last = p.clone())
            .unwrap_or_else(|e| panic!("{format:?} level {level}: {e:#}"));
        assert_eq!(last.items_done, last.items_total);
        assert_eq!(last.bytes_done, last.bytes_total);
        assert!(last.bytes_total >= 300_000);
        assert_eq!(detect(&archive), Some(format));

        let mut last = ArchiveProgress::default();
        let root = extract(&archive, &out_dir, &AtomicBool::new(false), |p| last = p.clone())
            .unwrap_or_else(|e| panic!("{format:?} extract: {e:#}"));
        assert_eq!(root, out_dir.join("project"));
        assert_same_tree(&src, &root);
        assert_eq!(last.bytes_done, last.bytes_total, "{format:?} extract progress");
        assert_eq!(only_entries(&out_dir), vec!["project"]);

        // Second extraction gets a unique name.
        let root2 = extract(&archive, &out_dir, &AtomicBool::new(false), noop).unwrap();
        assert_eq!(root2, out_dir.join("project copy"));
    }

    #[test]
    fn roundtrip_every_multi_format_default_level() {
        for f in ArchiveFormat::ALL.into_iter().filter(|f| f.supports_multiple()) {
            roundtrip_folder(f, f.default_level());
        }
    }

    #[test]
    fn roundtrip_min_and_max_levels() {
        for f in ArchiveFormat::ALL.into_iter().filter(|f| f.supports_multiple()) {
            if let Some(r) = f.level_range() {
                roundtrip_folder(f, *r.start());
                roundtrip_folder(f, *r.end());
            }
        }
    }

    #[test]
    fn roundtrip_single_file_every_format() {
        for f in ArchiveFormat::ALL {
            let levels = f.level_range().map_or(vec![0], |r| vec![*r.start(), *r.end()]);
            for level in levels {
                let tmp = TempDir::new().unwrap();
                let src = fixture(tmp.path()).join("sub/b.bin");
                let out_dir = tmp.path().join("out");
                fs::create_dir(&out_dir).unwrap();
                let archive = tmp.path().join(suggested_name(std::slice::from_ref(&src), f));
                assert!(archive.to_string_lossy().ends_with(&format!("b.bin.{}", f.extension())));
                compress(std::slice::from_ref(&src), &archive, f, level, &AtomicBool::new(false), noop)
                    .unwrap_or_else(|e| panic!("{f:?}: {e:#}"));
                let root = extract(&archive, &out_dir, &AtomicBool::new(false), noop)
                    .unwrap_or_else(|e| panic!("{f:?}: {e:#}"));
                assert_eq!(root, out_dir.join("b.bin"), "{f:?}");
                assert_eq!(fs::read(&src).unwrap(), fs::read(&root).unwrap(), "{f:?}");
            }
        }
    }

    #[test]
    fn multiple_inputs_extract_into_named_folder() {
        let tmp = TempDir::new().unwrap();
        let src = fixture(tmp.path());
        let inputs = vec![src.join("a.txt"), src.join("sub")];
        for f in ArchiveFormat::ALL.into_iter().filter(|f| f.supports_multiple()) {
            let archive = tmp.path().join(suggested_name(&inputs, f));
            assert!(archive.file_name().unwrap().to_string_lossy().starts_with("Archive."));
            compress(&inputs, &archive, f, f.default_level(), &AtomicBool::new(false), noop).unwrap();
            let out_dir = tmp.path().join(format!("out-{}", f.extension()));
            fs::create_dir(&out_dir).unwrap();
            let root = extract(&archive, &out_dir, &AtomicBool::new(false), noop).unwrap();
            assert_eq!(root, out_dir.join("Archive"), "{f:?}");
            assert_same_tree(&src.join("a.txt"), &root.join("a.txt"));
            assert_same_tree(&src.join("sub"), &root.join("sub"));
            assert_eq!(only_entries(&out_dir), vec!["Archive"]);
        }
    }

    #[test]
    fn single_stream_formats_reject_folders_and_multiple_inputs() {
        let tmp = TempDir::new().unwrap();
        let src = fixture(tmp.path());
        for f in [ArchiveFormat::Gz, ArchiveFormat::Br, ArchiveFormat::Zst] {
            let dest = tmp.path().join(format!("x.{}", f.extension()));
            assert!(compress(std::slice::from_ref(&src), &dest, f, 1, &AtomicBool::new(false), noop).is_err());
            assert!(compress(&[src.join("a.txt"), src.join("run.sh")], &dest, f, 1, &AtomicBool::new(false), noop).is_err());
            assert!(!dest.exists());
        }
    }

    #[test]
    fn cancellation_leaves_nothing_behind() {
        for f in ArchiveFormat::ALL {
            let tmp = TempDir::new().unwrap();
            let src = fixture(tmp.path());
            let input = if f.supports_multiple() { src.clone() } else { src.join("sub/b.bin") };
            let out = tmp.path().join("out");
            fs::create_dir(&out).unwrap();
            let archive = out.join(suggested_name(std::slice::from_ref(&input), f));

            // Cancelled mid-way: the flag flips at the first progress report,
            // after the temp file exists.
            let cancel = AtomicBool::new(false);
            let err = compress(std::slice::from_ref(&input), &archive, f, f.default_level(), &cancel, |_| {
                cancel.store(true, Ordering::Relaxed);
            })
            .unwrap_err();
            assert_eq!(err.to_string(), CANCELLED, "{f:?}");
            assert!(only_entries(&out).is_empty(), "{f:?} left {:?}", only_entries(&out));

            // Extraction cancelled up front.
            compress(std::slice::from_ref(&input), &archive, f, f.default_level(), &AtomicBool::new(false), noop).unwrap();
            let ex = tmp.path().join("ex");
            fs::create_dir(&ex).unwrap();
            let err = extract(&archive, &ex, &AtomicBool::new(true), noop).unwrap_err();
            assert_eq!(err.to_string(), CANCELLED, "{f:?}");
            assert!(only_entries(&ex).is_empty(), "{f:?}");

            // Extraction cancelled mid-way.
            let cancel = AtomicBool::new(false);
            let err = extract(&archive, &ex, &cancel, |_| {
                cancel.store(true, Ordering::Relaxed);
            })
            .unwrap_err();
            assert_eq!(err.to_string(), CANCELLED, "{f:?}");
            assert!(only_entries(&ex).is_empty(), "{f:?}");
        }
    }

    #[test]
    fn zip_slip_is_rejected() {
        use zip::write::{SimpleFileOptions, ZipWriter};
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("deep/out");
        fs::create_dir_all(&out).unwrap();
        for evil in ["../evil.txt", "ok/../../evil.txt", "/tmp/rile-evil.txt", "..\\evil.txt", "C:/evil.txt"] {
            let archive = tmp.path().join("evil.zip");
            let mut zw = ZipWriter::new(File::create(&archive).unwrap());
            zw.start_file("fine.txt", SimpleFileOptions::default()).unwrap();
            zw.write_all(b"fine").unwrap();
            zw.start_file(evil, SimpleFileOptions::default()).unwrap();
            zw.write_all(b"pwned").unwrap();
            zw.finish().unwrap();

            let err = extract(&archive, &out, &AtomicBool::new(false), noop).unwrap_err();
            assert!(err.to_string().contains("unsafe path"), "{evil}: {err:#}");
            assert!(only_entries(&out).is_empty(), "{evil}");
            assert!(!tmp.path().join("deep/evil.txt").exists());
            assert!(!tmp.path().join("evil.txt").exists());
        }
        assert!(!Path::new("/tmp/rile-evil.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn tar_symlinks_cannot_escape() {
        use tar::{Builder, EntryType, Header};
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let out = tmp.path().join("out");
        fs::create_dir(&out).unwrap();

        let link = |b: &mut Builder<File>, path: &str, target: &str| {
            let mut h = Header::new_gnu();
            h.set_entry_type(EntryType::Symlink);
            h.set_size(0);
            h.set_mode(0o777);
            b.append_link(&mut h, path, target).unwrap();
        };
        let file = |b: &mut Builder<File>, path: &str| {
            let mut h = Header::new_gnu();
            h.set_size(5);
            h.set_mode(0o644);
            b.append_data(&mut h, path, &b"pwned"[..]).unwrap();
        };

        // An escaping link is skipped; the following file lands in a real dir.
        let archive = tmp.path().join("a.tar");
        let mut b = Builder::new(File::create(&archive).unwrap());
        link(&mut b, "pkg/esc", "../../outside");
        link(&mut b, "pkg/abs", outside.to_str().unwrap());
        link(&mut b, "pkg/ok", "sub/x");
        file(&mut b, "pkg/esc/x.txt");
        b.into_inner().unwrap();
        let root = extract(&archive, &out, &AtomicBool::new(false), noop).unwrap();
        assert!(fs::symlink_metadata(root.join("esc")).unwrap().is_dir());
        assert!(fs::symlink_metadata(root.join("abs")).is_err());
        assert_eq!(fs::read_link(root.join("ok")).unwrap(), Path::new("sub/x"));
        assert!(only_entries(&outside).is_empty());

        // Writing through an (in-tree) symlink is refused outright.
        let archive = tmp.path().join("b.tar");
        let mut b = Builder::new(File::create(&archive).unwrap());
        link(&mut b, "pkg/inner", ".");
        file(&mut b, "pkg/inner/x.txt");
        b.into_inner().unwrap();
        let out2 = tmp.path().join("out2");
        fs::create_dir(&out2).unwrap();
        assert!(extract(&archive, &out2, &AtomicBool::new(false), noop).is_err());
        assert!(only_entries(&out2).is_empty());
    }

    #[test]
    fn detect_and_names() {
        let d = |s: &str| detect(Path::new(s));
        assert_eq!(d("a.zip"), Some(ArchiveFormat::Zip));
        assert_eq!(d("a.7z"), Some(ArchiveFormat::SevenZ));
        assert_eq!(d("a.tar"), Some(ArchiveFormat::Tar));
        assert_eq!(d("a.TAR.GZ"), Some(ArchiveFormat::TarGz));
        assert_eq!(d("a.tgz"), Some(ArchiveFormat::TarGz));
        assert_eq!(d("a.tar.br"), Some(ArchiveFormat::TarBr));
        assert_eq!(d("a.tar.zst"), Some(ArchiveFormat::TarZst));
        assert_eq!(d("a.tzst"), Some(ArchiveFormat::TarZst));
        assert_eq!(d("a.txt.gz"), Some(ArchiveFormat::Gz));
        assert_eq!(d("a.br"), Some(ArchiveFormat::Br));
        assert_eq!(d("a.zst"), Some(ArchiveFormat::Zst));
        assert_eq!(d("a.txt"), None);
        assert_eq!(suggested_name(&[PathBuf::from("/x/foo")], ArchiveFormat::Zip), "foo.zip");
        assert_eq!(suggested_name(&[PathBuf::from("/x/foo"), PathBuf::from("/x/bar")], ArchiveFormat::TarZst), "Archive.tar.zst");
        assert_eq!(ArchiveFormat::Tar.level_range(), None);
        for f in ArchiveFormat::ALL {
            if let Some(r) = f.level_range() {
                assert!(r.contains(&f.default_level()));
            }
        }
    }
}
