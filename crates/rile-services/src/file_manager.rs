//! Opt-in macOS folder and Reveal-in-Finder associations. Never changes defaults
//! on launch, and never edits LaunchServices' private preference database.

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub can_set_rile: bool,
    pub is_default: bool,
}

#[cfg(not(target_os = "macos"))]
pub fn status() -> Status {
    Status::default()
}

/// Called on the main thread. macOS may ask for consent asynchronously.
#[cfg(not(target_os = "macos"))]
pub fn set_default(_rile: bool, complete: impl FnOnce(anyhow::Result<()>) + Send + 'static) {
    complete(Err(anyhow::anyhow!("Default file manager integration is only available on macOS")));
}

#[cfg(target_os = "macos")]
pub use mac::{set_default, status};

#[cfg(target_os = "macos")]
mod mac {
    use std::{path::Path, sync::Mutex};

    use anyhow::{Context, Result, anyhow, bail};
    use objc2::{rc::Retained, sel};
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSError, NSGlobalDomain, NSObjectProtocol, NSString, NSURL, NSUserDefaults};

    use super::Status;

    fn rile_bundle() -> Option<(Retained<NSURL>, String)> {
        let bundle = NSBundle::mainBundle();
        let path = bundle.bundlePath().to_string();
        if !Path::new(&path).extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
            return None;
        }
        Some((bundle.bundleURL(), bundle.bundleIdentifier()?.to_string()))
    }

    fn folder_url() -> Retained<NSURL> {
        // A directory with public.folder type; do not use an app bundle/package.
        NSURL::fileURLWithPath(&NSString::from_str("/Users"))
    }

    fn folder_handler() -> Option<String> {
        let url = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&folder_url())?;
        NSBundle::bundleWithURL(&url)?.bundleIdentifier().map(|id| id.to_string())
    }

    fn viewer() -> Option<String> {
        // Read the global domain, not Rile's application-specific preferences.
        let domain = NSUserDefaults::standardUserDefaults().persistentDomainForName(unsafe { NSGlobalDomain })?;
        domain.objectForKey(&NSString::from_str("NSFileViewer"))?
            .downcast_ref::<NSString>().map(|s| s.to_string())
    }

    pub fn status() -> Status {
        objc2::rc::autoreleasepool(|_| {
            let bundle = rile_bundle();
            Status {
                can_set_rile: bundle.is_some(),
                is_default: bundle.is_some_and(|(_, id)| folder_handler().as_deref() == Some(&id) && viewer().as_deref() == Some(&id)),
            }
        })
    }

    fn set_viewer(id: &str) -> Result<()> {
        // Use the preferences tool, not direct plist writes (cfprefsd caches them).
        let output = std::process::Command::new("/usr/bin/defaults")
            .args(["write", "-g", "NSFileViewer", "-string", id])
            .output().context("Couldn't update the default file viewer")?;
        if !output.status.success() {
            bail!("Couldn't update the default file viewer: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }

    pub fn set_default(rile: bool, complete: impl FnOnce(Result<()>) + Send + 'static) {
        let setup = (|| -> Result<_> {
            let workspace = NSWorkspace::sharedWorkspace();
            if !workspace.respondsToSelector(sel!(setDefaultApplicationAtURL:toOpenContentTypeOfFileAtURL:completionHandler:)) {
                bail!("Changing the default file manager requires macOS 12 or later");
            }
            let (url, id) = if rile {
                rile_bundle().context("Launch the installed Rile.app, rather than running Rile from cargo or a standalone binary")?
            } else {
                let id = "com.apple.finder".to_string();
                let url = workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(&id))
                    .context("Couldn't locate Finder")?;
                (url, id)
            };
            Ok((workspace, url, id))
        })();
        let (workspace, url, id) = match setup {
            Ok(setup) => setup,
            Err(error) => { complete(Err(error)); return; }
        };
        let complete = Mutex::new(Some(complete));
        let block = block2::RcBlock::new(move |error: *mut NSError| {
            let Some(complete) = complete.lock().unwrap().take() else { return };
            // NSError is borrowed only for the duration of this completion block.
            let result = if let Some(error) = unsafe { error.as_ref() } {
                Err(anyhow!("macOS didn't change the folder handler: {}", error.localizedDescription()))
            } else if folder_handler().as_deref() != Some(&id) {
                Err(anyhow!("macOS kept the existing folder handler"))
            } else {
                set_viewer(&id).context("Folder opening changed, but Reveal in Finder did not. Retry this action")
            };
            complete(result);
        });
        workspace.setDefaultApplicationAtURL_toOpenContentTypeOfFileAtURL_completionHandler(&url, &folder_url(), Some(&block));
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn standalone_binary_cannot_become_default() {
            // No preference writes or consent prompts in tests.
            assert!(rile_bundle().is_none());
            assert!(!status().can_set_rile);
            let (tx, rx) = std::sync::mpsc::channel();
            set_default(true, move |result| tx.send(result).unwrap());
            assert!(rx.recv().unwrap().is_err());
        }
    }
}
