//! Secrets (passwords / key passphrases) stored in the OS keychain via `keyring`.
//! Nothing is ever written to disk by this module.

use anyhow::{Context, Result};

/// Keychain service name under which all Rile remote secrets are stored.
pub const SERVICE: &str = "dev.rile.remote";

fn entry(id: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, id).context("failed to open keychain entry")
}

/// Store (or replace) the secret for connection `id`.
pub fn save(id: &str, secret: &str) -> Result<()> {
    entry(id)?
        .set_password(secret)
        .context("failed to save secret to the keychain")
}

/// Load the secret for connection `id`; `Ok(None)` if none is stored.
pub fn load(id: &str) -> Result<Option<String>> {
    match entry(id)?.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("failed to read secret from the keychain"),
    }
}

/// Delete the secret for connection `id` (no error if none is stored).
pub fn delete(id: &str) -> Result<()> {
    match entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e).context("failed to delete secret from the keychain"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Touches the real OS keychain (may prompt), so ignored by default.
    #[test]
    #[ignore]
    fn roundtrip() {
        let id = format!("rile-test-{}", crate::ConnectionInfo::new_id());
        assert_eq!(load(&id).unwrap(), None);
        save(&id, "s3cret ✓").unwrap();
        assert_eq!(load(&id).unwrap().as_deref(), Some("s3cret ✓"));
        save(&id, "other").unwrap();
        assert_eq!(load(&id).unwrap().as_deref(), Some("other"));
        delete(&id).unwrap();
        assert_eq!(load(&id).unwrap(), None);
        delete(&id).unwrap();
    }
}
