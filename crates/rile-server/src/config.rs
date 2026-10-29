//! `/etc/rile-server/config.toml`: what the server publishes.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rile_remote::peer::{DEFAULT_PORT, Share};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    #[default]
    ReadOnly,
    ReadWrite,
}

impl Access {
    pub fn label(self) -> &'static str {
        match self {
            Access::ReadOnly => "read-only",
            Access::ReadWrite => "read-write",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShareConfig {
    /// Name Rile shows for the folder (unique).
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub access: Access,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    /// Name other devices see; empty uses the host name.
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Where files sent with "Send to" are saved; none declines them.
    #[serde(default)]
    pub inbox: Option<PathBuf>,
    #[serde(default, rename = "share")]
    pub shares: Vec<ShareConfig>,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for Config {
    fn default() -> Self {
        Self { name: String::new(), port: DEFAULT_PORT, inbox: None, shares: Vec::new() }
    }
}

impl Config {
    /// Reads `path`; a missing file is an empty configuration.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Validates, then writes atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let text = format!(
            "# Rile Server. Edit with `rilectl`, or edit by hand and run `systemctl reload rile-server`.\n{}",
            toml::to_string_pretty(self)?
        );
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        for (i, s) in self.shares.iter().enumerate() {
            check_name(&s.name)?;
            if self.shares[..i].iter().any(|o| o.name.eq_ignore_ascii_case(&s.name)) {
                bail!("Two shared folders are named “{}”", s.name);
            }
            if !s.path.is_absolute() {
                bail!("{} is not an absolute path", s.path.display());
            }
        }
        Ok(())
    }

    /// `base`, or `base 2`, `base 3`, … if a share already has that name.
    pub fn unique_name(&self, base: &str) -> String {
        let base = clean_name(base);
        let taken = |n: &str| self.shares.iter().any(|s| s.name.eq_ignore_ascii_case(n));
        if !taken(&base) {
            return base;
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap_or(base)
    }

    pub fn share(&self, name: &str) -> Option<&ShareConfig> {
        self.shares.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// The shares to publish: existing folders only.
    pub fn published(&self) -> Vec<Share> {
        self.shares
            .iter()
            .filter(|s| s.path.is_dir())
            .map(|s| Share { name: s.name.clone(), path: s.path.clone(), writable: s.access == Access::ReadWrite })
            .collect()
    }

    /// Shares whose folder is missing (shown as warnings).
    pub fn missing(&self) -> Vec<&ShareConfig> {
        self.shares.iter().filter(|s| !s.path.is_dir()).collect()
    }
}

/// A share name Rile can show: no `/`, not empty, not `.`/`..`.
pub fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        bail!("“{name}” can't be used as a share name (no slashes, not empty)");
    }
    Ok(())
}

/// Turns a folder name into a share name.
pub fn clean_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if matches!(c, '/' | '\\' | '\0') { '-' } else { c }).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." { "Shared".into() } else { cleaned }
}

/// The share name for `path`: its folder name (the host name for `/`).
pub fn default_name(path: &Path, host: &str) -> String {
    match path.file_name() {
        Some(n) => clean_name(&n.to_string_lossy()),
        None => clean_name(host),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)] // uses Unix absolute paths
    fn round_trips_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("etc/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());

        let mut c = Config { name: "Box".into(), inbox: Some("/srv/in".into()), ..Default::default() };
        c.shares.push(ShareConfig { name: "Media".into(), path: dir.path().to_path_buf(), access: Access::ReadWrite });
        c.shares.push(ShareConfig { name: "Gone".into(), path: "/nonexistent/rile".into(), access: Access::ReadOnly });
        c.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.published().len(), 1);
        assert!(back.published()[0].writable);
        assert_eq!(back.missing().len(), 1);
        assert_eq!(back.unique_name("media"), "media 2");
        assert_eq!(back.unique_name("a/b"), "a-b");

        c.shares.push(ShareConfig { name: "MEDIA".into(), path: "/tmp".into(), access: Access::ReadOnly });
        assert!(c.save(&path).is_err());
        c.shares.pop();
        c.shares.push(ShareConfig { name: "x/y".into(), path: "/tmp".into(), access: Access::ReadOnly });
        assert!(c.validate().is_err());
    }

    #[test]
    fn parses_hand_written_files() {
        let c: Config = toml::from_str("[[share]]\nname = \"Root\"\npath = \"/\"\n").unwrap();
        assert_eq!((c.port, c.shares[0].access), (DEFAULT_PORT, Access::ReadOnly));
        assert_eq!(default_name(Path::new("/"), "nas"), "nas");
        assert_eq!(default_name(Path::new("/srv/media"), "nas"), "media");
    }
}
