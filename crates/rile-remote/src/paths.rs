//! Helpers for `/`-separated remote paths (independent of the local OS).

/// Join `name` onto `base`. An absolute `name` replaces `base`. The result is normalized.
pub fn join(base: &str, name: &str) -> String {
    if name.starts_with('/') || base.is_empty() {
        return normalize(name);
    }
    if name.is_empty() {
        return normalize(base);
    }
    normalize(&format!("{base}/{name}"))
}

/// Parent directory, or `None` for the root (or a single relative component).
pub fn parent(path: &str) -> Option<String> {
    let norm = normalize(path);
    if norm == "/" || norm == "." {
        return None;
    }
    match norm.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(i) => Some(norm[..i].to_string()),
        None => None,
    }
}

/// Last component (`""` for the root).
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

/// Collapse duplicate slashes, resolve `.` and `..`, drop trailing slashes.
/// `..` never climbs above `/` for absolute paths; an empty path becomes `/`.
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if absolute => {}
                _ => parts.push(".."),
            },
            c => parts.push(c),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cases() {
        assert_eq!(normalize(""), "/");
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("//a//b/"), "/a/b");
        assert_eq!(normalize("/a/./b/../c"), "/a/c");
        assert_eq!(normalize("/../.."), "/");
        assert_eq!(normalize("/a/.."), "/");
        assert_eq!(normalize("a/b/.."), "a");
        assert_eq!(normalize("a/../.."), "..");
        assert_eq!(normalize("../x"), "../x");
        assert_eq!(normalize("./"), ".");
        assert_eq!(normalize("/with space/ünï"), "/with space/ünï");
    }

    #[test]
    fn join_cases() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(join("/a/", "b/"), "/a/b");
        assert_eq!(join("/a", "/b"), "/b");
        assert_eq!(join("/a", ".."), "/");
        assert_eq!(join("/a", ""), "/a");
        assert_eq!(join("", "x"), "x");
        assert_eq!(join("rel", "x"), "rel/x");
    }

    #[test]
    fn parent_cases() {
        assert_eq!(parent("/"), None);
        assert_eq!(parent(""), None);
        assert_eq!(parent("/a"), Some("/".into()));
        assert_eq!(parent("/a/b/"), Some("/a".into()));
        assert_eq!(parent("a/b"), Some("a".into()));
        assert_eq!(parent("a"), None);
    }

    #[test]
    fn file_name_cases() {
        assert_eq!(file_name("/"), "");
        assert_eq!(file_name("/a"), "a");
        assert_eq!(file_name("/a/b.txt"), "b.txt");
        assert_eq!(file_name("/a/dir/"), "dir");
        assert_eq!(file_name("plain"), "plain");
    }
}
