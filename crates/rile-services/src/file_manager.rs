//! Opt-in folder associations: macOS folder and Reveal-in-Finder handlers,
//! Windows Explorer verbs. Never changes defaults on launch, and never edits
//! LaunchServices' private preference database.

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub can_set_rile: bool,
    pub is_default: bool,
    pub is_viewer: bool,
}

#[derive(Debug)]
pub enum Outcome {
    Updated,
    ViewerOnly,
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn status() -> Status {
    Status::default()
}

/// Called on the main thread. macOS may ask for consent asynchronously.
#[cfg(not(any(target_os = "macos", windows)))]
pub fn set_default(_rile: bool, complete: impl FnOnce(anyhow::Result<Outcome>) + Send + 'static) {
    complete(Err(anyhow::anyhow!("Default file manager integration is only available on macOS and Windows")));
}

#[cfg(target_os = "macos")]
pub use mac::{set_default, status};

#[cfg(windows)]
pub use win::{register_shell_integration, set_default, status, unregister_shell_integration};

/// Per-user (HKCU) Explorer integration: the `rile://` protocol, "Open in
/// Rile" on folders, drives and folder backgrounds, and, only when the user
/// asks for it, Rile as the default verb so double-clicking a folder opens
/// Rile. Win+E and the desktop keep using Explorer either way.
#[cfg(windows)]
mod win {
    use anyhow::{Context, Result};
    use windows_registry::CURRENT_USER;

    use super::{Outcome, Status};

    const CLASSES: &str = r"Software\Classes";
    const VERB: &str = "rile";
    /// Folder classes that get the verb, with the placeholder Explorer fills in.
    const TARGETS: [(&str, &str); 3] = [("Directory", "%1"), ("Drive", "%1"), (r"Directory\Background", "%V")];
    /// Classes whose default verb decides what opening a folder does.
    const DEFAULT_TARGETS: [&str; 2] = ["Directory", "Drive"];

    fn exe() -> Result<String> {
        let exe = dunce::canonicalize(std::env::current_exe()?)?;
        Ok(exe.to_string_lossy().into_owned())
    }

    /// Writes the protocol and context-menu verbs for the running executable.
    /// Idempotent; called at startup so a moved or updated install stays
    /// registered.
    pub fn register_shell_integration() -> Result<()> {
        let exe = exe()?;
        let protocol = CURRENT_USER.create(format!(r"{CLASSES}\rile"))?;
        protocol.set_string("", "URL:Rile")?;
        protocol.set_string("URL Protocol", "")?;
        protocol.create("DefaultIcon")?.set_string("", format!("\"{exe}\",0"))?;
        protocol.create(r"shell\open\command")?.set_string("", format!("\"{exe}\" \"%1\""))?;
        for (class, arg) in TARGETS {
            let verb = CURRENT_USER.create(format!(r"{CLASSES}\{class}\shell\{VERB}"))?;
            verb.set_string("", "Open in Rile")?;
            verb.set_string("Icon", format!("\"{exe}\",0"))?;
            verb.create("command")?.set_string("", format!("\"{exe}\" \"{arg}\""))?;
        }
        notify_shell();
        Ok(())
    }

    /// Removes everything [`register_shell_integration`] and [`set_default`]
    /// wrote (used by the uninstaller via `rile --unregister`).
    pub fn unregister_shell_integration() -> Result<()> {
        set_default_verb(false)?;
        CURRENT_USER.remove_tree(format!(r"{CLASSES}\rile")).ok();
        for (class, _) in TARGETS {
            CURRENT_USER.remove_tree(format!(r"{CLASSES}\{class}\shell\{VERB}")).ok();
        }
        notify_shell();
        Ok(())
    }

    fn is_default_verb(class: &str) -> bool {
        CURRENT_USER
            .open(format!(r"{CLASSES}\{class}\shell"))
            .and_then(|k| k.get_string(""))
            .is_ok_and(|v| v == VERB)
    }

    fn set_default_verb(rile: bool) -> Result<()> {
        for class in DEFAULT_TARGETS {
            let shell = CURRENT_USER.create(format!(r"{CLASSES}\{class}\shell"))?;
            if rile {
                shell.set_string("", VERB)?;
            } else if is_default_verb(class) {
                // Without our value Explorer falls back to the machine-wide default.
                shell.remove_value("").ok();
            }
        }
        Ok(())
    }

    fn notify_shell() {
        use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }

    pub fn status() -> Status {
        let registered = CURRENT_USER
            .open(format!(r"{CLASSES}\Directory\shell\{VERB}\command"))
            .and_then(|k| k.get_string(""))
            .is_ok_and(|cmd| exe().is_ok_and(|exe| cmd.contains(&exe)));
        Status {
            can_set_rile: exe().is_ok(),
            is_default: registered && DEFAULT_TARGETS.iter().all(|c| is_default_verb(c)),
            is_viewer: registered,
        }
    }

    pub fn set_default(rile: bool, complete: impl FnOnce(Result<Outcome>) + Send + 'static) {
        let result = (|| {
            register_shell_integration().context("Registering Rile with Explorer")?;
            set_default_verb(rile).context("Changing the default folder action")?;
            notify_shell();
            Ok(Outcome::Updated)
        })();
        complete(result);
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::{path::Path, sync::Mutex};

    use anyhow::{Context, Result, anyhow, bail};
    use objc2::{rc::Retained, sel};
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSError, NSGlobalDomain, NSObjectProtocol, NSString, NSURL, NSUserDefaults, NSUnderlyingErrorKey};
    use objc2_uniform_type_identifiers::{UTType, UTTypeFolder};

    use super::{Outcome, Status};

    fn rile_bundle() -> Option<(Retained<NSURL>, String)> {
        let bundle = NSBundle::mainBundle();
        let path = bundle.bundlePath().to_string();
        if !Path::new(&path).extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
            return None;
        }
        Some((bundle.bundleURL(), bundle.bundleIdentifier()?.to_string()))
    }

    fn folder_type() -> &'static UTType {
        // Address the registered folder type directly rather than deriving it
        // through a sample filesystem item such as the protected /Users folder.
        // SAFETY: UniformTypeIdentifiers provides an immutable system constant.
        unsafe { UTTypeFolder }
    }

    fn folder_handler() -> Option<String> {
        let workspace = NSWorkspace::sharedWorkspace();
        if !workspace.respondsToSelector(sel!(URLForApplicationToOpenContentType:)) { return None; }
        let url = workspace.URLForApplicationToOpenContentType(folder_type())?;
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
                can_set_rile: bundle.is_some() && NSWorkspace::sharedWorkspace()
                    .respondsToSelector(sel!(setDefaultApplicationAtURL:toOpenContentType:completionHandler:)),
                is_default: bundle.as_ref().is_some_and(|(_, id)| folder_handler().as_deref() == Some(id) && viewer().as_deref() == Some(id)),
                is_viewer: bundle.as_ref().is_some_and(|(_, id)| viewer().as_deref() == Some(id)),
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

    fn folder_binding_rejected(error: &NSError) -> bool {
        // Recent macOS versions reject public.folder as an invalid bindable,
        // even when assigning Finder to itself. Do not treat consent denial or
        // arbitrary failures as permission to apply a partial change.
        if error.domain().to_string() != "NSCocoaErrorDomain" || error.code() != 256 { return false; }
        let info = error.userInfo();
        info.objectForKey(unsafe { NSUnderlyingErrorKey })
            .and_then(|error| error.downcast_ref::<NSError>().map(|error|
                error.domain().to_string() == "NSOSStatusErrorDomain" && error.code() == -50))
            .unwrap_or(false)
    }

    pub fn set_default(rile: bool, complete: impl FnOnce(Result<Outcome>) + Send + 'static) {
        let setup = (|| -> Result<_> {
            let workspace = NSWorkspace::sharedWorkspace();
            if !workspace.respondsToSelector(sel!(setDefaultApplicationAtURL:toOpenContentType:completionHandler:)) {
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
                if folder_binding_rejected(error) {
                    set_viewer(&id).map(|()| Outcome::ViewerOnly)
                        .context("macOS rejected the folder association, and the file viewer preference could not be updated")
                } else {
                    Err(anyhow!("macOS didn't change the folder handler: {} ({} {}); details: {:?}", error.localizedDescription(), error.domain(), error.code(), error.userInfo()))
                }
            } else if folder_handler().as_deref() != Some(&id) {
                Err(anyhow!("macOS kept the existing folder handler"))
            } else {
                set_viewer(&id).context("Folder opening changed, but Reveal in Finder did not. Retry this action")
                    .map(|()| Outcome::Updated)
            };
            complete(result);
        });
        workspace.setDefaultApplicationAtURL_toOpenContentType_completionHandler(&url, folder_type(), Some(&block));
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_invalid_folder_binding_allows_partial_integration() {
            use objc2::runtime::AnyObject;
            use objc2_foundation::NSDictionary;

            for (domain, code, expected) in [
                ("NSOSStatusErrorDomain", -50, true),
                ("NSOSStatusErrorDomain", -128, false), // user cancelled
                ("NSCocoaErrorDomain", -50, false),
            ] {
                // SAFETY: the underlying error dictionary has NSString keys and NSError values.
                let underlying = unsafe { NSError::errorWithDomain_code_userInfo(&NSString::from_str(domain), code, None) };
                let info = NSDictionary::from_slices(&[unsafe { NSUnderlyingErrorKey }], &[&*underlying as &AnyObject]);
                let error = unsafe { NSError::errorWithDomain_code_userInfo(&NSString::from_str("NSCocoaErrorDomain"), 256, Some(&info)) };
                assert_eq!(folder_binding_rejected(&error), expected);
            }
            let error = unsafe { NSError::errorWithDomain_code_userInfo(&NSString::from_str("NSCocoaErrorDomain"), 256, None) };
            assert!(!folder_binding_rejected(&error));
        }

        #[test]
        fn association_targets_the_registered_folder_type() {
            assert_eq!(folder_type().identifier().to_string(), "public.folder");
            // Lookup needs no sample directory, filesystem access or writes.
            assert!(folder_handler().is_some());
        }

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
