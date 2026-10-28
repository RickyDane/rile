//! Streaming filesystem search without any index.
//!
//! The tree is walked with the `ignore` crate's parallel walker; names are
//! matched before any `stat` call, metadata filters run next, and content is
//! searched last (files only) with ripgrep's `grep-searcher`. Hits are
//! delivered through a callback as soon as they are found.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, anyhow, bail};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::sinks::Lossy;
use grep_searcher::{BinaryDetection, MmapChoice, Searcher, SearcherBuilder};
use ignore::{WalkBuilder, WalkState};
use rile_core::Entry;

/// Files at least this large are memory-mapped for content search. Smaller
/// files are read with buffered I/O, which is faster for many small files.
const MMAP_THRESHOLD: u64 = 4 * 1024 * 1024;
/// Maximum length (in chars) of the matching line returned in a hit.
const MAX_LINE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameMatch {
    /// Substring; whitespace also matches filename separators (`_`, `-`, `.`).
    /// An empty string matches everything. Use regex for literal whitespace.
    Substring(String),
    /// Shell glob matched against the file name (e.g. `*.rs`).
    Glob(String),
    /// Regular expression matched against the file name.
    Regex(String),
}

impl Default for NameMatch {
    fn default() -> Self {
        Self::Substring(String::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentMatch {
    pub pattern: String,
    /// When false the pattern is a literal string.
    pub regex: bool,
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindFilter {
    #[default]
    Any,
    Files,
    Dirs,
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub root: PathBuf,
    pub name: NameMatch,
    pub case_sensitive: bool,
    pub content: Option<ContentMatch>,
    pub include_hidden: bool,
    pub respect_gitignore: bool,
    pub max_depth: Option<usize>,
    pub kind: KindFilter,
    /// Lowercase extensions without the dot; empty means any.
    pub extensions: Vec<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_after: Option<SystemTime>,
    pub modified_before: Option<SystemTime>,
    /// Stop after this many hits; 0 means unlimited.
    pub max_results: usize,
}

impl SearchQuery {
    /// A query matching everything under `root` with sensible defaults.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            name: NameMatch::default(),
            case_sensitive: false,
            content: None,
            include_hidden: false,
            respect_gitignore: true,
            max_depth: None,
            kind: KindFilter::Any,
            extensions: Vec::new(),
            min_size: None,
            max_size: None,
            modified_after: None,
            modified_before: None,
            max_results: 10_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub entry: Entry,
    /// First matching content line: (1-based line number, trimmed text).
    pub line: Option<(u64, String)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchStats {
    /// Directory entries visited.
    pub scanned: u64,
    /// Hits delivered.
    pub matched: u64,
    pub elapsed: Duration,
}

pub(crate) enum NameMatcher {
    All,
    /// Needle is lowercased when matching case-insensitively.
    Substring { needle: String, case_sensitive: bool },
    Glob(globset::GlobMatcher),
    Regex(regex::Regex),
}

impl NameMatcher {
    pub(crate) fn new(m: &NameMatch, case_sensitive: bool) -> Result<Self> {
        Ok(match m {
            NameMatch::Substring(s) if s.is_empty() => Self::All,
            NameMatch::Substring(s) if s.chars().any(char::is_whitespace) && !s.trim().is_empty() => {
                let pattern = s.split_whitespace().map(regex::escape).collect::<Vec<_>>().join(r"[\s_.-]+");
                Self::Regex(regex::RegexBuilder::new(&pattern).case_insensitive(!case_sensitive).build()?)
            }
            NameMatch::Substring(s) => Self::Substring {
                needle: if case_sensitive { s.clone() } else { s.to_lowercase() },
                case_sensitive,
            },
            NameMatch::Glob(g) if g.is_empty() => Self::All,
            NameMatch::Glob(g) => Self::Glob(
                globset::GlobBuilder::new(g)
                    .case_insensitive(!case_sensitive)
                    .literal_separator(false)
                    .backslash_escape(true)
                    .build()
                    .map_err(|e| anyhow!("Invalid glob: {}", e.kind()))?
                    .compile_matcher(),
            ),
            NameMatch::Regex(r) if r.is_empty() => Self::All,
            NameMatch::Regex(r) => Self::Regex(
                regex::RegexBuilder::new(r)
                    .case_insensitive(!case_sensitive)
                    .build()
                    .map_err(|e| anyhow!("Invalid regular expression: {e}"))?,
            ),
        })
    }

    pub(crate) fn is_match(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Substring { needle, case_sensitive: true } => name.contains(needle.as_str()),
            Self::Substring { needle, case_sensitive: false } => {
                if name.is_ascii() && needle.is_ascii() {
                    contains_ascii_ci(name.as_bytes(), needle.as_bytes())
                } else {
                    name.to_lowercase().contains(needle.as_str())
                }
            }
            Self::Glob(g) => g.is_match(name),
            Self::Regex(r) => r.is_match(name),
        }
    }
}

/// Allocation-free ASCII case-insensitive substring test; `needle` must
/// already be lowercase.
fn contains_ascii_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.windows(needle.len())
        .any(|w| w.iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

fn content_matcher(c: &ContentMatch) -> Result<RegexMatcher> {
    if c.pattern.is_empty() {
        bail!("Content pattern is empty");
    }
    RegexMatcherBuilder::new()
        .case_insensitive(!c.case_sensitive)
        .fixed_strings(!c.regex)
        .build(&c.pattern)
        .map_err(|e| anyhow!("Invalid content pattern: {e}"))
}

/// Checks the query for errors (bad regex/glob, missing root, inverted
/// ranges) and returns a message suitable for showing in the UI.
pub fn validate(q: &SearchQuery) -> Result<()> {
    NameMatcher::new(&q.name, q.case_sensitive)?;
    if let Some(c) = &q.content {
        content_matcher(c)?;
    }
    if !q.root.is_dir() {
        bail!("Search location does not exist: {}", q.root.display());
    }
    if let (Some(lo), Some(hi)) = (q.min_size, q.max_size)
        && lo > hi
    {
        bail!("Minimum size is larger than maximum size");
    }
    if let (Some(a), Some(b)) = (q.modified_after, q.modified_before)
        && a > b
    {
        bail!("'Modified after' is later than 'modified before'");
    }
    Ok(())
}

/// Per-thread content searcher state.
struct ContentSearch<'a> {
    matcher: &'a RegexMatcher,
    buffered: Searcher,
    mmapped: Searcher,
}

impl<'a> ContentSearch<'a> {
    fn new(matcher: &'a RegexMatcher) -> Self {
        let build = |mmap: bool| {
            let mut b = SearcherBuilder::new();
            b.binary_detection(BinaryDetection::quit(0)).line_number(true);
            if mmap {
                // SAFETY: the map is read-only and only used for the duration
                // of a single search. A file truncated concurrently could
                // SIGBUS; grep-searcher's `auto` only maps regular files and
                // we only map large ones, matching ripgrep's own trade-off.
                b.memory_map(unsafe { MmapChoice::auto() });
            }
            b.build()
        };
        Self { matcher, buffered: build(false), mmapped: build(true) }
    }

    /// First matching line, or None (also None for binary/unreadable files).
    fn first_match(&mut self, path: &Path, size: u64, cancel: &AtomicBool) -> Option<(u64, String)> {
        let searcher = if size >= MMAP_THRESHOLD { &mut self.mmapped } else { &mut self.buffered };
        let mut found = None;
        let sink = Lossy(|lnum, line| {
            found = Some((lnum, clip_line(line)));
            Ok(false)
        });
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        searcher.search_path(self.matcher, path, sink).ok()?;
        found
    }
}

fn clip_line(line: &str) -> String {
    let t = line.trim();
    match t.char_indices().nth(MAX_LINE_CHARS) {
        Some((i, _)) => format!("{}…", &t[..i]),
        None => t.to_string(),
    }
}

/// Walks `q.root` in parallel and calls `on_hit` (from worker threads) for
/// every match. Returns when the walk finishes, `max_results` is reached or
/// `cancel` is set.
pub fn search(
    q: &SearchQuery,
    cancel: &AtomicBool,
    on_hit: impl Fn(SearchHit) + Sync + Send,
) -> Result<SearchStats> {
    let start = Instant::now();
    validate(q)?;
    let name = NameMatcher::new(&q.name, q.case_sensitive)?;
    let content = q.content.as_ref().map(content_matcher).transpose()?;
    let exts: Vec<String> = q
        .extensions
        .iter()
        .map(|e| e.trim_start_matches('.').to_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    // Directories never have content, so a content query implies files.
    let kind = if content.is_some() { KindFilter::Files } else { q.kind };
    let max = if q.max_results == 0 { u64::MAX } else { q.max_results as u64 };
    let needs_meta = q.min_size.is_some()
        || q.max_size.is_some()
        || q.modified_after.is_some()
        || q.modified_before.is_some();

    let scanned = AtomicU64::new(0);
    let matched = AtomicU64::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(12);

    let mut wb = WalkBuilder::new(&q.root);
    wb.standard_filters(false)
        .hidden(!q.include_hidden)
        .git_ignore(q.respect_gitignore)
        .git_global(q.respect_gitignore)
        .git_exclude(q.respect_gitignore)
        .ignore(q.respect_gitignore)
        .parents(q.respect_gitignore)
        .follow_links(false)
        .max_depth(q.max_depth)
        .threads(threads);
    if q.respect_gitignore {
        // Repository internals are never interesting when honouring ignores.
        wb.filter_entry(|e| !(e.file_name() == ".git" && e.file_type().is_some_and(|t| t.is_dir())));
    }

    let (name, content, exts, on_hit) = (&name, content.as_ref(), &exts, &on_hit);
    let (scanned, matched) = (&scanned, &matched);

    wb.build_parallel().run(|| {
        let mut cs = content.map(ContentSearch::new);
        Box::new(move |res| {
            if cancel.load(Ordering::Relaxed) || matched.load(Ordering::Relaxed) >= max {
                return WalkState::Quit;
            }
            let Ok(dent) = res else { return WalkState::Continue };
            if dent.depth() == 0 {
                return WalkState::Continue;
            }
            scanned.fetch_add(1, Ordering::Relaxed);

            // Cheap checks first: no syscalls.
            let Some(ft) = dent.file_type() else { return WalkState::Continue };
            let fname = dent.file_name().to_string_lossy();
            let looks_dir = ft.is_dir();
            match kind {
                KindFilter::Files if looks_dir => return WalkState::Continue,
                KindFilter::Dirs if !looks_dir && !ft.is_symlink() => return WalkState::Continue,
                _ => {}
            }
            if !exts.is_empty() {
                if looks_dir {
                    return WalkState::Continue;
                }
                let ext = Path::new(fname.as_ref())
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase());
                if !ext.is_some_and(|e| exts.contains(&e)) {
                    return WalkState::Continue;
                }
            }
            if !name.is_match(&fname) {
                return WalkState::Continue;
            }

            // Now stat.
            let Some(entry) = Entry::from_path(dent.into_path()) else { return WalkState::Continue };
            if !q.include_hidden && entry.hidden {
                return WalkState::Continue;
            }
            match kind {
                KindFilter::Files if entry.is_dir => return WalkState::Continue,
                KindFilter::Dirs if !entry.is_dir => return WalkState::Continue,
                _ => {}
            }
            if needs_meta && !meta_ok(q, &entry) {
                return WalkState::Continue;
            }
            let line = match cs.as_mut() {
                Some(cs) => match cs.first_match(&entry.path, entry.size, cancel) {
                    Some(l) => Some(l),
                    None => return WalkState::Continue,
                },
                None => None,
            };
            if matched.fetch_add(1, Ordering::Relaxed) >= max {
                return WalkState::Quit;
            }
            on_hit(SearchHit { entry, line });
            WalkState::Continue
        })
    });

    Ok(SearchStats {
        scanned: scanned.load(Ordering::Relaxed),
        matched: matched.load(Ordering::Relaxed).min(max),
        elapsed: start.elapsed(),
    })
}

pub(crate) fn meta_ok(q: &SearchQuery, e: &Entry) -> bool {
    // Size limits only make sense for files.
    if !e.is_dir {
        if q.min_size.is_some_and(|m| e.size < m) || q.max_size.is_some_and(|m| e.size > m) {
            return false;
        }
    } else if q.min_size.is_some() || q.max_size.is_some() {
        return false;
    }
    if q.modified_after.is_some() || q.modified_before.is_some() {
        let Some(t) = e.modified else { return false };
        if q.modified_after.is_some_and(|a| t < a) || q.modified_before.is_some_and(|b| t > b) {
            return false;
        }
    }
    true
}

/// Queries the OS search index: Spotlight via `mdfind` on macOS (`query` is
/// passed verbatim, so Spotlight syntax like `kMDItemDisplayName == "*x*"`
/// works), Windows Search on Windows (Advanced Query Syntax). This uses the
/// OS-maintained index, not one of ours. Returns an error elsewhere.
pub fn spotlight_search(
    query: &str,
    root: Option<&Path>,
    max: usize,
    cancel: &AtomicBool,
) -> Result<Vec<PathBuf>> {
    #[cfg(target_os = "macos")]
    {
        spotlight_macos(query, root, max, cancel)
    }
    #[cfg(windows)]
    {
        if query.trim().is_empty() {
            bail!("Windows Search query is empty");
        }
        let max = if max == 0 { usize::MAX } else { max };
        let mut out = Vec::new();
        crate::win::windows_search(query, root, cancel, |p| {
            out.push(p);
            out.len() < max
        })?;
        Ok(out)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (query, root, max, cancel);
        bail!("Spotlight search is only available on macOS")
    }
}

#[cfg(target_os = "macos")]
fn spotlight_macos(query: &str, root: Option<&Path>, max: usize, cancel: &AtomicBool) -> Result<Vec<PathBuf>> {
    use std::io::{BufRead, BufReader, Read};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    if query.trim().is_empty() {
        bail!("Spotlight query is empty");
    }
    let mut cmd = Command::new("/usr/bin/mdfind");
    cmd.arg("-0");
    if let Some(r) = root {
        cmd.arg("-onlyin").arg(r);
    }
    cmd.arg(query).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let mut child = anyhow::Context::context(cmd.spawn(), "Failed to start mdfind")?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    let (tx, rx) = mpsc::channel::<PathBuf>();
    let reader = std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        let mut buf = Vec::new();
        while r.read_until(0, &mut buf).is_ok_and(|n| n > 0) {
            if buf.last() == Some(&0) {
                buf.pop();
            }
            if !buf.is_empty() {
                use std::os::unix::ffi::OsStrExt;
                let p = PathBuf::from(std::ffi::OsStr::from_bytes(&buf));
                if tx.send(p).is_err() {
                    break;
                }
            }
            buf.clear();
        }
    });

    let max = if max == 0 { usize::MAX } else { max };
    let mut out = Vec::new();
    let mut stopped = false;
    loop {
        if cancel.load(Ordering::Relaxed) || out.len() >= max {
            stopped = true;
            let _ = child.kill();
            break;
        }
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(p) => out.push(p),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(rx);
    let status = child.wait()?;
    let _ = reader.join();
    if !stopped && !status.success() {
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err);
        bail!("mdfind failed: {}", err.trim());
    }
    out.truncate(max);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        fs::create_dir_all(p.join("src/nested")).unwrap();
        fs::create_dir_all(p.join(".hidden_dir")).unwrap();
        fs::create_dir_all(p.join("target")).unwrap();
        fs::write(p.join("src/main.rs"), "fn main() {\n    println!(\"Hello World\");\n}\n").unwrap();
        fs::write(p.join("src/lib.rs"), "pub fn add() {}\n").unwrap();
        fs::write(p.join("src/nested/Report2.TXT"), "alpha\nbeta needle gamma\n").unwrap();
        fs::write(p.join("src/nested/report10.txt"), "nothing here").unwrap();
        fs::write(p.join(".hidden_dir/secret.txt"), "needle").unwrap();
        fs::write(p.join("target/built.txt"), "needle").unwrap();
        fs::write(p.join("blob.bin"), b"needle\0\0\0binary").unwrap();
        fs::write(p.join("big.dat"), vec![b'x'; 5000]).unwrap();
        // .gitignore only applies inside a git repository.
        fs::create_dir(p.join(".git")).unwrap();
        fs::write(p.join(".gitignore"), "target/\n").unwrap();
        t
    }

    fn run(q: &SearchQuery) -> (Vec<SearchHit>, SearchStats) {
        let hits = Mutex::new(Vec::new());
        let stats = search(q, &AtomicBool::new(false), |h| hits.lock().unwrap().push(h)).unwrap();
        let mut v = hits.into_inner().unwrap();
        v.sort_by(|a, b| a.entry.path.cmp(&b.entry.path));
        (v, stats)
    }

    fn names(h: &[SearchHit]) -> Vec<String> {
        let mut n: Vec<String> = h.iter().map(|h| h.entry.name.to_string()).collect();
        n.sort();
        n
    }

    #[test]
    fn substring_case_insensitive() {
        let t = tree();
        let mut q = SearchQuery::new(t.path());
        q.name = NameMatch::Substring("REPORT".into());
        let (h, s) = run(&q);
        assert_eq!(names(&h), ["Report2.TXT", "report10.txt"]);
        assert_eq!(s.matched, 2);
        q.case_sensitive = true;
        assert!(run(&q).0.is_empty());
    }

    #[test]
    fn phrase_matches_filename_separators_without_broadening_literal_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["professor_layton.nds", "Professor-Layton.sav", "professor.layton.txt", "professor layton.pdf", "professor_unrelated_layton.nds", "layton_professor.nds"] {
            fs::write(tmp.path().join(name), "").unwrap();
        }
        let mut q = SearchQuery::new(tmp.path());
        q.name = NameMatch::Substring("Professor Layton".into());
        assert_eq!(run(&q).0.len(), 4);
        q.case_sensitive = true;
        assert_eq!(names(&run(&q).0), ["Professor-Layton.sav"]);
        q.case_sensitive = false;
        q.name = NameMatch::Substring("professor_layton".into());
        assert_eq!(names(&run(&q).0), ["professor_layton.nds"]);
        q.name = NameMatch::Regex("professor layton".into());
        assert_eq!(names(&run(&q).0), ["professor layton.pdf"]);
        let literal = NameMatcher::new(&NameMatch::Substring("a+b c".into()), false).unwrap();
        assert!(literal.is_match("a+b_c.txt"));
        assert!(!literal.is_match("aaab_c.txt"));
    }

    #[test]
    fn empty_substring_matches_all_visible_and_respects_gitignore() {
        let t = tree();
        let q = SearchQuery::new(t.path());
        let n = names(&run(&q).0);
        assert!(n.contains(&"main.rs".to_string()));
        assert!(!n.iter().any(|x| x == "secret.txt" || x == "built.txt" || x == "target"));
        let mut q2 = q.clone();
        q2.include_hidden = true;
        q2.respect_gitignore = false;
        let n2 = names(&run(&q2).0);
        assert!(n2.contains(&"secret.txt".to_string()));
        assert!(n2.contains(&"built.txt".to_string()));
    }

    #[test]
    fn glob_regex_kind_ext() {
        let t = tree();
        let mut q = SearchQuery::new(t.path());
        q.name = NameMatch::Glob("*.rs".into());
        assert_eq!(names(&run(&q).0), ["lib.rs", "main.rs"]);
        q.name = NameMatch::Regex(r"^report\d+\.txt$".into());
        assert_eq!(names(&run(&q).0), ["Report2.TXT", "report10.txt"]);
        q.name = NameMatch::default();
        q.kind = KindFilter::Dirs;
        assert_eq!(names(&run(&q).0), ["nested", "src"]);
        q.kind = KindFilter::Any;
        q.extensions = vec!["txt".into()];
        assert_eq!(names(&run(&q).0), ["Report2.TXT", "report10.txt"]);
        q.max_depth = Some(1);
        assert!(run(&q).0.is_empty());
    }

    #[test]
    fn size_and_time_filters() {
        let t = tree();
        let mut q = SearchQuery::new(t.path());
        q.min_size = Some(4000);
        assert_eq!(names(&run(&q).0), ["big.dat"]);
        q.min_size = None;
        q.modified_after = Some(SystemTime::now() + Duration::from_secs(3600));
        assert!(run(&q).0.is_empty());
        q.modified_after = Some(SystemTime::UNIX_EPOCH);
        assert!(!run(&q).0.is_empty());
    }

    #[test]
    fn content_search_skips_binary() {
        let t = tree();
        let mut q = SearchQuery::new(t.path());
        q.content = Some(ContentMatch { pattern: "NEEDLE".into(), regex: false, case_sensitive: false });
        let (h, _) = run(&q);
        assert_eq!(names(&h), ["Report2.TXT"]);
        assert_eq!(h[0].line, Some((2, "beta needle gamma".into())));
        q.content = Some(ContentMatch { pattern: r"println!\(".into(), regex: true, case_sensitive: true });
        let (h, _) = run(&q);
        assert_eq!(names(&h), ["main.rs"]);
        assert_eq!(h[0].line.as_ref().unwrap().0, 2);
    }

    #[test]
    fn max_results_and_cancel() {
        let t = tree();
        let mut q = SearchQuery::new(t.path());
        q.max_results = 2;
        let (h, s) = run(&q);
        assert_eq!(h.len(), 2);
        assert_eq!(s.matched, 2);
        let cancel = AtomicBool::new(true);
        let n = AtomicU64::new(0);
        search(&q, &cancel, |_| {
            n.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
        assert_eq!(n.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn validate_reports_errors() {
        let t = tree();
        let mut q = SearchQuery::new(t.path());
        q.name = NameMatch::Regex("(".into());
        assert!(validate(&q).unwrap_err().to_string().contains("regular expression"));
        q.name = NameMatch::Glob("[a".into());
        assert!(validate(&q).unwrap_err().to_string().contains("glob"));
        q.name = NameMatch::default();
        q.content = Some(ContentMatch { pattern: "[".into(), regex: true, case_sensitive: false });
        assert!(validate(&q).is_err());
        q.content = Some(ContentMatch { pattern: "[".into(), regex: false, case_sensitive: false });
        assert!(validate(&q).is_ok());
        q.root = t.path().join("missing");
        assert!(validate(&q).is_err());
    }

    #[test]
    fn clip_line_limits_chars() {
        let long = "é".repeat(300);
        let c = clip_line(&format!("   {long}  "));
        assert_eq!(c.chars().count(), MAX_LINE_CHARS + 1);
        assert!(contains_ascii_ci(b"Hello World", b"o w"));
        assert!(!contains_ascii_ci(b"Hello", b"xyz"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn spotlight_unsupported() {
        assert!(spotlight_search("x", None, 10, &AtomicBool::new(false)).is_err());
    }
}
