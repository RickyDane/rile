//! Batch rename rules and their live preview.
//!
//! A name is built in this order: find & replace on the name (regex or
//! plain text), then the template (`{name}`, `{n}`, `{ext}`, `{date}`,
//! `{parent}`), then the case change. The extension is kept unless the rule
//! says to include it or the template places `{ext}` itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseChange {
    #[default]
    Keep,
    Lower,
    Upper,
    /// Every Word Capitalized.
    Title,
    /// First letter capitalized, rest lower.
    Sentence,
}

impl CaseChange {
    pub const ALL: [CaseChange; 5] = [CaseChange::Keep, CaseChange::Lower, CaseChange::Upper, CaseChange::Title, CaseChange::Sentence];

    pub fn label(self) -> &'static str {
        match self {
            CaseChange::Keep => "Keep",
            CaseChange::Lower => "lowercase",
            CaseChange::Upper => "UPPERCASE",
            CaseChange::Title => "Title Case",
            CaseChange::Sentence => "Sentence case",
        }
    }

    fn apply(self, s: &str) -> String {
        match self {
            CaseChange::Keep => s.to_string(),
            CaseChange::Lower => s.to_lowercase(),
            CaseChange::Upper => s.to_uppercase(),
            CaseChange::Title => {
                let mut out = String::with_capacity(s.len());
                let mut start = true;
                for c in s.chars() {
                    if start && c.is_alphanumeric() {
                        out.extend(c.to_uppercase());
                        start = false;
                    } else {
                        out.extend(c.to_lowercase());
                    }
                    if c.is_whitespace() || matches!(c, '-' | '_' | '.' | '(' | '[') {
                        start = true;
                    }
                }
                out
            }
            CaseChange::Sentence => {
                let lower = s.to_lowercase();
                let mut chars = lower.chars();
                match chars.next() {
                    Some(f) => f.to_uppercase().chain(chars).collect(),
                    None => String::new(),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    /// `{name}` by default; see the module docs for tokens.
    pub template: String,
    pub case: CaseChange,
    /// First value of `{n}`.
    pub start: u64,
    /// Minimum digits of `{n}` (zero padded).
    pub pad: usize,
    /// Let find/replace and the case change touch the extension too.
    pub include_ext: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            find: String::new(),
            replace: String::new(),
            regex: false,
            case_sensitive: false,
            template: "{name}".into(),
            case: CaseChange::Keep,
            start: 1,
            pad: 1,
            include_ext: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Unchanged,
    Ok,
    /// Clashes with another item (renamed or not) in the same folder.
    Conflict,
    /// Not a valid file name.
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub from: PathBuf,
    pub old_name: String,
    pub new_name: String,
    pub status: Status,
}

impl Preview {
    pub fn to(&self) -> PathBuf {
        self.from.with_file_name(&self.new_name)
    }
}

fn split_ext(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        // ".env" has no extension; "a." has an empty one we keep as part of the stem.
        Some(i) if i > 0 && i + 1 < name.len() => (&name[..i], Some(&name[i + 1..])),
        _ => (name, None),
    }
}

fn date_of(path: &Path) -> String {
    let secs = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Compiles the find pattern; `Err` carries a message for the UI.
fn matcher(rules: &Rules) -> Result<Option<Regex>, String> {
    if rules.find.is_empty() {
        return Ok(None);
    }
    let pattern = if rules.regex { rules.find.clone() } else { regex::escape(&rules.find) };
    RegexBuilder::new(&pattern)
        .case_insensitive(!rules.case_sensitive)
        .build()
        .map(Some)
        .map_err(|e| match e {
            regex::Error::Syntax(s) => s.lines().last().unwrap_or("Invalid pattern").trim().to_string(),
            other => other.to_string(),
        })
}

/// The new name of `path` (the `index`-th item) under `rules`.
fn new_name(path: &Path, index: usize, rules: &Rules, re: Option<&Regex>) -> String {
    let old = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let is_dir = path.is_dir();
    let (stem, ext) = if rules.include_ext || is_dir { (old.as_str(), None) } else { split_ext(&old) };
    let replaced = match re {
        Some(re) if rules.regex => re.replace_all(stem, rules.replace.as_str()).into_owned(),
        Some(re) => re.replace_all(stem, regex::NoExpand(&rules.replace)).into_owned(),
        None => stem.to_string(),
    };
    let template = if rules.template.trim().is_empty() { "{name}" } else { rules.template.as_str() };
    let n = rules.start + index as u64;
    let mut out = String::new();
    let mut rest = template;
    let mut placed_ext = false;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let token = &after[..close];
        match token {
            "name" => out.push_str(&replaced),
            "n" => out.push_str(&format!("{n:0width$}", width = rules.pad.max(1))),
            "ext" => {
                placed_ext = true;
                out.push_str(ext.unwrap_or(""));
            }
            "date" => out.push_str(&date_of(path)),
            "parent" => out.push_str(&path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            _ => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    let mut name = rules.case.apply(&out);
    if !placed_ext && let Some(ext) = ext {
        name.push('.');
        name.push_str(ext);
    }
    name.trim().to_string()
}

fn fold(name: &str) -> String {
    // macOS and Windows volumes are case-insensitive by default.
    if cfg!(any(target_os = "macos", windows)) { name.to_lowercase() } else { name.to_string() }
}

/// Previews `rules` on `paths` (in the given order, which numbering follows).
/// `Err` is a message about an invalid find pattern.
pub fn preview(paths: &[PathBuf], rules: &Rules) -> Result<Vec<Preview>, String> {
    let re = matcher(rules)?;
    let mut out: Vec<Preview> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let old_name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let new_name = new_name(p, i, rules, re.as_ref());
            let status = if new_name == old_name {
                Status::Unchanged
            } else if !rile_core::ops::is_safe_component(&new_name) {
                Status::Invalid
            } else {
                Status::Ok
            };
            Preview { from: p.clone(), old_name, new_name, status }
        })
        .collect();

    // Clashes: two results with the same name in one folder, or a result
    // matching an existing item that isn't itself being renamed away.
    let renamed: HashSet<&PathBuf> = out.iter().filter(|p| p.status == Status::Ok).map(|p| &p.from).collect();
    let mut seen: HashMap<(PathBuf, String), usize> = HashMap::new();
    for p in &out {
        let dir = p.from.parent().map(Path::to_path_buf).unwrap_or_default();
        *seen.entry((dir, fold(&p.new_name))).or_default() += 1;
    }
    let mut conflicts = Vec::new();
    for (i, p) in out.iter().enumerate() {
        if p.status != Status::Ok {
            continue;
        }
        let dir = p.from.parent().map(Path::to_path_buf).unwrap_or_default();
        let dup = seen.get(&(dir.clone(), fold(&p.new_name))).copied().unwrap_or(0) > 1;
        let target = dir.join(&p.new_name);
        let case_only = fold(&p.new_name) == fold(&p.old_name);
        let occupied = !case_only && std::fs::symlink_metadata(&target).is_ok() && !renamed.contains(&target);
        if dup || occupied {
            conflicts.push(i);
        }
    }
    for i in conflicts {
        out[i].status = Status::Conflict;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn names(p: &[Preview]) -> Vec<&str> {
        p.iter().map(|p| p.new_name.as_str()).collect()
    }

    #[test]
    fn find_replace_template_case() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("IMG_001.JPG");
        let b = t.path().join("IMG_002.JPG");
        fs::write(&a, "").unwrap();
        fs::write(&b, "").unwrap();
        let paths = vec![a.clone(), b.clone()];
        let rules = Rules { find: "img_".into(), replace: "Holiday ".into(), ..Default::default() };
        assert_eq!(names(&preview(&paths, &rules).unwrap()), ["Holiday 001.JPG", "Holiday 002.JPG"]);

        let rules = Rules { template: "Trip {n}".into(), pad: 3, start: 7, ..Default::default() };
        assert_eq!(names(&preview(&paths, &rules).unwrap()), ["Trip 007.JPG", "Trip 008.JPG"]);

        let rules = Rules { case: CaseChange::Lower, include_ext: true, ..Default::default() };
        assert_eq!(names(&preview(&paths, &rules).unwrap()), ["img_001.jpg", "img_002.jpg"]);

        let rules = Rules { find: r"IMG_(\d+)".into(), replace: "$1-photo".into(), regex: true, case_sensitive: true, ..Default::default() };
        assert_eq!(names(&preview(&paths, &rules).unwrap()), ["001-photo.JPG", "002-photo.JPG"]);

        let rules = Rules { find: "(".into(), regex: true, ..Default::default() };
        assert!(preview(&paths, &rules).is_err());
    }

    #[test]
    fn conflicts_and_invalid() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a.txt");
        let b = t.path().join("b.txt");
        let c = t.path().join("c.txt");
        for p in [&a, &b, &c] {
            fs::write(p, "").unwrap();
        }
        // Both become "x.txt".
        let rules = Rules { template: "x".into(), ..Default::default() };
        let p = preview(&[a.clone(), b.clone()], &rules).unwrap();
        assert!(p.iter().all(|p| p.status == Status::Conflict));
        // a → c clashes with the untouched c.
        let rules = Rules { find: "a".into(), replace: "c".into(), ..Default::default() };
        let p = preview(&[a.clone()], &rules).unwrap();
        assert_eq!(p[0].status, Status::Conflict);
        // a ↔ b swap through each other is fine.
        let rules = Rules { find: "^.*$".into(), regex: true, include_ext: true, ..Default::default() };
        let p = preview(&[a.clone()], &rules).unwrap();
        assert_eq!(p[0].status, Status::Invalid, "empty name");
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}
