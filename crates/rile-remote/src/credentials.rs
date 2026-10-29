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

/// Load only if the OS can grant access without asking the user. Startup must
/// never turn a saved connection into an unsolicited authentication dialog.
#[cfg(target_os = "macos")]
pub fn load_noninteractive(id: &str) -> Result<Option<String>> {
    use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
    use security_framework::os::macos::keychain::SecKeychain;

    let results = ItemSearchOptions::new()
        .keychains(&[SecKeychain::default()?])
        .class(ItemClass::generic_password())
        .service(SERVICE)
        .account(id)
        .load_data(true)
        .skip_authenticated_items(true)
        .search();
    match results {
        Ok(items) => items.into_iter().find_map(|item| match item {
            SearchResult::Data(bytes) => Some(String::from_utf8(bytes).map(Some).context("invalid saved secret")),
            _ => None,
        }).unwrap_or(Ok(None)),
        // Not found, locked or awaiting authorization: leave manual connect available.
        Err(_) => Ok(None),
    }
}

/// Other backends don't expose a guaranteed non-interactive read through
/// keyring. Use session credentials there rather than risking an unlock prompt.
#[cfg(not(target_os = "macos"))]
pub fn load_noninteractive(_id: &str) -> Result<Option<String>> {
    Ok(None)
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
        #[cfg(target_os = "macos")]
        assert_eq!(load_noninteractive(&id).unwrap().as_deref(), Some("s3cret ✓"));
        save(&id, "other").unwrap();
        assert_eq!(load(&id).unwrap().as_deref(), Some("other"));
        delete(&id).unwrap();
        assert_eq!(load(&id).unwrap(), None);
        assert_eq!(load_noninteractive(&id).unwrap(), None);
        delete(&id).unwrap();
    }
}
