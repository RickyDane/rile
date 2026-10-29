//! Routes macOS open/reveal requests without opening documents in other apps.

use std::path::PathBuf;

use gpui_kit::App;
use rile_core::history::Location;

#[derive(Clone, Default)]
pub struct Requests(std::rc::Rc<std::cell::RefCell<RequestState>>);

#[derive(Default)]
struct RequestState {
    app: Option<gpui_kit::AsyncApp>,
    pending: Vec<(Vec<PathBuf>, bool)>,
    /// `rile://` links that arrived before the app was ready.
    links: Vec<AppLink>,
}

/// Pages other processes (the backup menu bar helper) can open via `rile://`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppLink {
    /// `rile://backups`
    Backups,
    /// `rile://backup-settings`
    BackupSettings,
}

impl AppLink {
    pub fn parse(url: &str) -> Option<Self> {
        match url.strip_prefix("rile://")?.trim_end_matches('/') {
            "backups" => Some(Self::Backups),
            "backup-settings" => Some(Self::BackupSettings),
            _ => None,
        }
    }
}

impl Requests {
    pub fn dispatch(&self, paths: Vec<PathBuf>, reveal: bool) {
        let mut state = self.0.borrow_mut();
        if let Some(cx) = state.app.as_mut() {
            cx.update(|cx| open(paths, reveal, cx));
        } else {
            state.pending.push((paths, reveal));
        }
    }

    pub fn dispatch_link(&self, link: AppLink) {
        let mut state = self.0.borrow_mut();
        if let Some(cx) = state.app.as_mut() {
            cx.update(|cx| crate::show_backups(link == AppLink::BackupSettings, cx));
        } else {
            state.links.push(link);
        }
    }

    pub fn ready(&self, cx: &mut App) {
        let (pending, links) = {
            let mut state = self.0.borrow_mut();
            state.app = Some(cx.to_async());
            (std::mem::take(&mut state.pending), std::mem::take(&mut state.links))
        };
        for (paths, reveal) in pending { open(paths, reveal, cx); }
        for link in links { crate::show_backups(link == AppLink::BackupSettings, cx); }
    }
}

pub fn file_paths(urls: Vec<String>) -> Vec<PathBuf> {
    urls.into_iter().filter_map(|s| url::Url::parse(&s).ok()?.to_file_path().ok())
        .map(|path| {
            #[cfg(target_os = "macos")]
            let path = mac::filesystem_path(&path).unwrap_or(path);
            path
        }).collect()
}

// Keep selections from the same directory together, including multi-item reveals.
fn targets(paths: Vec<PathBuf>, reveal: bool) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut targets: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for path in paths {
        if !path.exists() { continue; }
        let select = reveal || !path.is_dir();
        let dir = if select { path.parent().map(PathBuf::from) } else { Some(path.clone()) };
        let Some(dir) = dir else { continue };
        let index = targets.iter().position(|(d, _)| *d == dir).unwrap_or_else(|| {
            targets.push((dir, Vec::new()));
            targets.len() - 1
        });
        if select && !targets[index].1.contains(&path) {
            targets[index].1.push(path);
        }
    }
    targets
}

pub fn open(paths: Vec<PathBuf>, reveal: bool, cx: &mut App) {
    for (index, (dir, selection)) in targets(paths, reveal).into_iter().enumerate() {
        // Reuse the main browser even after a cold launch, rather than leaving an
        // empty Home window behind. Additional directories get their own windows.
        let existing = (index == 0).then(|| {
            cx.try_global::<crate::MainWindow>().and_then(|main| main.1.upgrade().map(|ws| (main.0, ws)))
        }).flatten();
        if let Some((handle, ws)) = existing {
            let opened = handle.update(cx, |_, window, cx| {
                ws.update(cx, |ws, cx| {
                    let same_directory = ws.pane().read(cx).location == Location::Dir(dir.clone());
                    ws.open_location(Location::Dir(dir.clone()), window, cx);
                    ws.pane().update(cx, |pane, cx| {
                        pane.select_after_reload(selection.clone());
                        // navigate() is a no-op for an already displayed folder.
                        if same_directory { pane.reload(cx); }
                    });
                });
                window.activate_window();
            });
            if opened.is_ok() { cx.activate(true); continue; }
        }
        match crate::open_workspace(Location::Dir(dir), None, cx) {
            Ok((_, ws)) => {
                ws.update(cx, |ws, cx| ws.pane().update(cx, |pane, _| pane.select_after_reload(selection)));
                cx.activate(true);
            }
            Err(error) => eprintln!("Couldn't open external file-manager request: {error}"),
        }
    }
}

#[cfg(target_os = "macos")]
pub mod mac {
    use std::ffi::{CStr, CString, OsString};
    use std::os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::ffi::{OsStrExt, OsStringExt}};
    use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, sel};
    use objc2_foundation::{MainThreadMarker, NSAppleEventDescriptor, NSAppleEventManager, NSObject};

    use super::*;

    pub(super) fn filesystem_path(path: &std::path::Path) -> Option<PathBuf> {
        // Apple Events may decompose Unicode even on APFS, which preserves the
        // original spelling. F_GETPATH restores the names used by pane entries.
        // O_SYMLINK keeps a revealed symlink selected, rather than its target.
        let path = CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: Valid NUL-terminated path; no variadic mode is needed without O_CREAT.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_EVTONLY | libc::O_SYMLINK | libc::O_CLOEXEC) };
        if fd < 0 { return None; }
        // SAFETY: We own this newly opened descriptor and close it on all paths.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut buffer = [0 as libc::c_char; libc::PATH_MAX as usize];
        // SAFETY: F_GETPATH writes a NUL-terminated path into a PATH_MAX buffer.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } < 0 { return None; }
        // SAFETY: A successful F_GETPATH guarantees NUL termination.
        let bytes = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_bytes().to_vec();
        Some(PathBuf::from(OsString::from_vec(bytes)))
    }

    struct HandlerIvars {
        reveal: Box<dyn Fn(Vec<PathBuf>)>,
    }

    define_class!(
        // SAFETY: NSObject has no subclass requirements. Access is main-thread only.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[name = "RileFileViewerEvents"]
        #[ivars = HandlerIvars]
        struct Handler;

        impl Handler {
            // SAFETY: Matches NSAppleEventManager's event/reply selector signature.
            #[unsafe(method(reveal:withReplyEvent:))]
            fn reveal(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
                if let Some(items) = event.paramDescriptorForKeyword(u32::from_be_bytes(*b"----")) {
                    (self.ivars().reveal)(descriptor_paths(&items));
                }
            }
        }
    );

    pub(crate) fn descriptor_paths(items: &NSAppleEventDescriptor) -> Vec<PathBuf> {
        if items.descriptorType() == u32::from_be_bytes(*b"list") {
            return (1..=items.numberOfItems())
                .filter_map(|i| items.descriptorAtIndex(i))
                .flat_map(|item| descriptor_paths(&item)).collect();
        }
        // Coercion handles both legacy alias descriptors and file URLs.
        items.coerceToDescriptorType(u32::from_be_bytes(*b"furl"))
            .and_then(|d| String::from_utf8(d.data().to_vec()).ok())
            .map(|url| file_paths(vec![url])).unwrap_or_default()
    }

    pub struct FileViewerEvents { _handler: Retained<Handler> }

    pub fn install(reveal: impl Fn(Vec<PathBuf>) + 'static) -> FileViewerEvents {
        let handler = Handler::alloc(MainThreadMarker::new().expect("main thread"))
            .set_ivars(HandlerIvars { reveal: Box::new(reveal) });
        // SAFETY: Initialize the NSObject superclass before registering the handler.
        let handler: Retained<Handler> = unsafe { msg_send![super(handler), init] };
        // NSWorkspace Reveal in Finder uses the Finder suite's misc/mvis event.
        // SAFETY: Handler and selector have the required signature and lifetime.
        unsafe {
            NSAppleEventManager::sharedAppleEventManager().setEventHandler_andSelector_forEventClass_andEventID(
                &handler, sel!(reveal:withReplyEvent:), u32::from_be_bytes(*b"misc"), u32::from_be_bytes(*b"mvis"),
            );
        }
        FileViewerEvents { _handler: handler }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use objc2_foundation::{NSString, NSURL};

        #[test]
        fn reveal_decodes_file_urls_and_lists() {
            let dir = tempfile::tempdir().unwrap();
            let root = dunce::canonicalize(dir.path()).unwrap();
            std::fs::write(root.join("Rile file #ü"), "test").unwrap();
            // Preserve the spelling on disk, including composed Unicode on APFS.
            let path = std::fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
            let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
            let item = NSAppleEventDescriptor::descriptorWithFileURL(&url);
            assert_eq!(descriptor_paths(&item), vec![path.clone()]);
            let list = NSAppleEventDescriptor::listDescriptor();
            list.insertDescriptor_atIndex(&item, 1);
            assert_eq!(descriptor_paths(&list), vec![path]);
        }

        #[test]
        fn reveal_preserves_symlinks() {
            let dir = tempfile::tempdir().unwrap();
            let root = dunce::canonicalize(dir.path()).unwrap();
            let target = root.join("target");
            std::fs::write(&target, "test").unwrap();
            std::os::unix::fs::symlink(&target, root.join("shortcut ü")).unwrap();
            let link = std::fs::read_dir(&root).unwrap().map(Result::unwrap)
                .find(|e| e.file_type().unwrap().is_symlink()).unwrap().path();
            let url = NSURL::fileURLWithPath(&NSString::from_str(link.to_str().unwrap()));
            let item = NSAppleEventDescriptor::descriptorWithFileURL(&url);
            assert_eq!(descriptor_paths(&item), vec![link]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_local_file_urls_and_decodes_paths() {
        let (url, path) = if cfg!(windows) { ("file:///C:/tmp/a%20b%23%C3%BC", r"C:\tmp\a b#ü") } else { ("file:///tmp/a%20b%23%C3%BC", "/tmp/a b#ü") };
        assert_eq!(file_paths(vec![url.into(), "https://example.com".into(), "garbage".into()]), vec![PathBuf::from(path)]);
    }

    #[test]
    fn groups_file_reveals_and_distinguishes_folder_opening() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("folder");
        std::fs::create_dir(&dir).unwrap();
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        assert_eq!(targets(vec![a.clone(), b.clone(), a.clone()], false), vec![(dir.clone(), vec![a, b])]);
        assert_eq!(targets(vec![dir.clone()], false), vec![(dir.clone(), vec![])]);
        assert_eq!(targets(vec![dir.clone()], true), vec![(root.path().to_path_buf(), vec![dir])]);
        assert!(targets(vec![root.path().join("missing")], false).is_empty());
    }
}
