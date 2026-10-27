//! Headless UI integration tests: real workspace, real (temporary) files,
//! simulated keyboard input.

use std::fs;
use std::path::Path;
use std::time::Duration;

use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext, WindowOptions};
use rile_core::history::Location;

use crate::actions;
use crate::workspace::Workspace;

const TIMEOUT: Duration = Duration::from_secs(5);

fn setup(dir: &Path, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    // No network update checks or plugin compilation in tests.
    setup_with(dir, "update_check_on_start = false\nplugins_enabled = false\nfavorites = []\n", None, cx)
}

/// RILE_CONFIG_DIR is process-global: `setup` holds this until the workspace has
/// read it; tests that save and reload settings hold it across both.
fn config_lock() -> std::sync::MutexGuard<'static, ()> {
    static SETUP: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SETUP.lock().unwrap_or_else(|e| e.into_inner())
}

fn setup_with(
    dir: &Path,
    settings: &str,
    plugin: Option<&Path>,
    cx: &mut TestAppContext,
) -> (AnyWindowHandle, Entity<Workspace>) {
    let _guard = config_lock();
    let config = tempfile::tempdir().unwrap().keep();
    fs::write(config.join("settings.toml"), settings).unwrap();
    if let Some(plugin) = plugin {
        fs::create_dir_all(config.join("plugins")).unwrap();
        fs::copy(plugin, config.join("plugins/hello.wasm")).unwrap();
    }
    // SAFETY: tests in this module don't read the environment concurrently.
    unsafe { std::env::set_var("RILE_CONFIG_DIR", config) };
    crate::pane::WATCH_FS.store(false, std::sync::atomic::Ordering::Relaxed);
    let dir = dir.to_path_buf();
    cx.update(|cx| {
        gpui_kit::init(cx);
        actions::bind_keys(cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Workspace::new(Location::Dir(dir), window, cx))
        })
        .unwrap()
    })
}

async fn wait_for_names(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    ws: &Entity<Workspace>,
    expected: &[&str],
) {
    let _ = window;
    let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    let start = std::time::Instant::now();
    loop {
        // File work runs on real threads, so poll with wall-clock time.
        cx.run_until_parked();
        let names: Vec<String> = cx.update(|cx| {
            ws.read(cx).pane().read(cx).entries().iter().map(|e| e.name.to_string()).collect()
        });
        if names == expected {
            return;
        }
        assert!(start.elapsed() < TIMEOUT, "expected {expected:?}, got {names:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Polls (running the executor) until `check` passes.
fn wait_until(cx: &mut TestAppContext, what: &str, mut check: impl FnMut(&mut gpui_kit::App) -> bool) {
    let start = std::time::Instant::now();
    loop {
        cx.run_until_parked();
        if cx.update(|cx| check(cx)) {
            return;
        }
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn press(cx: &mut TestAppContext, window: AnyWindowHandle, keys: &[&str]) {
    for k in keys {
        cx.simulate_keystrokes(window, k);
        cx.run_until_parked();
    }
}

#[gpui_kit::test]
async fn copy_paste_rename_and_navigate(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "hello").unwrap();

    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["sub", "a.txt"]).await;

    // Select a.txt, copy, paste -> "a copy.txt" appears and gets selected.
    press(cx, window, &["down", "down", "cmd-c", "cmd-v"]);
    wait_for_names(cx, window, &ws, &["sub", "a copy.txt", "a.txt"]).await;
    assert_eq!(fs::read_to_string(root.join("a copy.txt")).unwrap(), "hello");
    let selected = cx.update(|cx| ws.read(cx).pane().read(cx).selected_paths());
    assert_eq!(selected, [root.join("a copy.txt")]);

    // Cut it and paste into "sub".
    press(cx, window, &["cmd-x", "home", "enter"]);
    wait_for_names(cx, window, &ws, &[]).await;
    press(cx, window, &["cmd-v"]);
    wait_for_names(cx, window, &ws, &["a copy.txt"]).await;
    assert!(!root.join("a copy.txt").exists());

    // Back to the parent, which preselects "sub".
    press(cx, window, &["cmd-["]);
    wait_for_names(cx, window, &ws, &["sub", "a.txt"]).await;
    let cursor = cx.update(|cx| ws.read(cx).pane().read(cx).cursor_entry().map(|e| e.name.to_string()));
    assert_eq!(cursor.as_deref(), Some("sub"));

    // New folder opens the rename dialog; typing a name + enter renames it.
    press(cx, window, &["cmd-shift-n"]);
    wait_for_names(cx, window, &ws, &["sub", "untitled folder", "a.txt"]).await;
    cx.simulate_input(window, "docs");
    press(cx, window, &["enter"]);
    wait_for_names(cx, window, &ws, &["docs", "sub", "a.txt"]).await;

    // Duplicate, then permanent delete with confirmation.
    cx.update(|cx| ws.update(cx, |ws, cx| {
        let pane = ws.pane().clone();
        pane.update(cx, |p, cx| p.cursor_to(2, false, cx));
    }));
    press(cx, window, &["cmd-d"]);
    wait_for_names(cx, window, &ws, &["docs", "sub", "a copy.txt", "a.txt"]).await;
    press(cx, window, &["cmd-shift-backspace", "enter"]);
    wait_for_names(cx, window, &ws, &["docs", "sub", "a.txt"]).await;
}

#[gpui_kit::test]
async fn paste_clipboard_images(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &[]).await;

    // Both PNG and JPEG clipboard data must produce real .jpeg files.
    for (format, name) in [
        (gpui_kit::ImageFormat::Png, "pasted image.jpeg"),
        (gpui_kit::ImageFormat::Jpeg, "pasted image copy.jpeg"),
    ] {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut bytes, image::ImageFormat::from_extension(format.extension()).unwrap())
            .unwrap();
        let bytes = bytes.into_inner();
        cx.update(|cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(
            &gpui_kit::Image::from_bytes(format, bytes.clone()),
        )));
        press(cx, window, &["cmd-v"]);
        wait_until(cx, "pasted image selection", |cx| {
            ws.read(cx).pane().read(cx).selected_paths() == [root.join(name)]
        });
        let saved = fs::read(root.join(name)).unwrap();
        assert_eq!(image::guess_format(&saved).unwrap(), image::ImageFormat::Jpeg);
        let decoded = image::load_from_memory(&saved).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
    }

    // Pasting again must not overwrite the existing image.
    let original = fs::read(root.join("pasted image.jpeg")).unwrap();
    press(cx, window, &["cmd-v"]);
    wait_for_names(cx, window, &ws, &["pasted image copy 2.jpeg", "pasted image copy.jpeg", "pasted image.jpeg"]).await;
    assert_eq!(fs::read(root.join("pasted image.jpeg")).unwrap(), original);
}

#[gpui_kit::test]
async fn paste_image_overrides_cut_files_in_current_directory(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "keep me").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["sub", "a.txt"]).await;
    press(cx, window, &["down", "down", "cmd-x", "home", "enter"]);
    wait_for_names(cx, window, &ws, &[]).await;

    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 2).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    cx.update(|cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_image(
        &gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, bytes.into_inner()),
    )));
    press(cx, window, &["cmd-v"]);
    wait_for_names(cx, window, &ws, &["pasted image.jpeg"]).await;
    let saved = image::open(root.join("sub/pasted image.jpeg")).unwrap().to_rgb8();
    assert_eq!(saved.get_pixel(0, 0).0, [255, 255, 255]);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "keep me");
    cx.update(|cx| {
        assert!(ws.read(cx).clipboard.items.is_none());
        assert!(ws.read(cx).pane().read(cx).cut_paths.is_empty());
    });
}

#[gpui_kit::test]
async fn pdf_page_layout_and_control_scroll_zoom(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, ScrollDelta, ScrollEvent, VisualTestContext, point, px};
    use std::sync::Arc;
    use crate::workspace::preview::Loaded;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let path = root.join("two-pages.pdf");
    fs::write(&path, "preview fixture").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["two-pages.pdf"]).await;
    press(cx, window, &["down", "space"]);
    wait_until(cx, "preview loaded", |cx| ws.read(cx).preview.loaded.is_some());

    // Inject two rasterized portrait pages, avoiding platform PDF dependencies.
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(600, 900).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let page = Arc::new(gpui_kit::Image::from_bytes(gpui_kit::ImageFormat::Png, png.into_inner()));
    cx.update(|cx| ws.update(cx, |ws, cx| {
        ws.preview.loaded = Some((path, Arc::new(Loaded::Pdf { pages: 2 }), Arc::new(Vec::new())));
        ws.preview.pdf_pages = vec![page.clone(), page];
        cx.notify();
    }));

    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let first = vcx.debug_bounds("pdf-page-0").unwrap();
    let second = vcx.debug_bounds("pdf-page-1").unwrap();
    assert!((f32::from(first.size.height) / f32::from(first.size.width) - 1.5).abs() < 0.01);
    assert!(second.top() >= first.bottom());
    assert!(f32::from(second.top() - first.bottom()) < 60., "pages should have only a small gap");

    let position = vcx.debug_bounds("pdf-pages").unwrap().center();
    let scroll = |y, control| ScrollEvent {
        position,
        delta: ScrollDelta::Lines(point(0., y)),
        modifiers: Modifiers { control, ..Modifiers::none() },
        ..Default::default()
    };
    vcx.simulate_event(scroll(1., true));
    vcx.run_until_parked();
    let enlarged = vcx.debug_bounds("pdf-page-0").unwrap();
    assert!(enlarged.size.width > first.size.width, "Ctrl+scroll up should zoom in");
    assert!(enlarged.size.width < first.size.width * 1.1, "wheel zoom should not snap to 25% steps");
    assert!((enlarged.origin.y - first.origin.y).abs() < px(1.), "zoom should not also scroll");
    vcx.simulate_event(scroll(-1., true));
    vcx.run_until_parked();
    let restored = vcx.debug_bounds("pdf-page-0").unwrap();
    assert!((restored.size.width - first.size.width).abs() < px(1.));
    vcx.simulate_event(scroll(-3., false));
    vcx.run_until_parked();
    let scrolled = vcx.debug_bounds("pdf-page-0").unwrap();
    assert_eq!(scrolled.size.width, restored.size.width);
    assert!(scrolled.origin.y < restored.origin.y, "ordinary scrolling should still move the pages");

    // Test away from the top edge, where clamping can hide accidental scrolling.
    vcx.simulate_event(scroll(-1., true));
    vcx.run_until_parked();
    let zoomed_out = vcx.debug_bounds("pdf-page-0").unwrap();
    assert!((zoomed_out.origin.y - scrolled.origin.y).abs() < px(1.), "Ctrl+scroll down must not scroll");
    vcx.simulate_event(scroll(1., true));
    vcx.run_until_parked();
    let zoomed_back = vcx.debug_bounds("pdf-page-0").unwrap();
    assert!((zoomed_back.origin.y - scrolled.origin.y).abs() < px(1.), "Ctrl+scroll up must not scroll");

    vcx.simulate_event(scroll(-2., true));
    vcx.run_until_parked();
    let half = vcx.debug_bounds("pdf-page-0").unwrap();
    let second = vcx.debug_bounds("pdf-page-1").unwrap();
    assert!(half.size.width < first.size.width);
    assert!((f32::from(half.size.height) / f32::from(half.size.width) - 1.5).abs() < 0.01);
    assert!(f32::from(second.top() - half.bottom()) < 60., "zooming out should not leave intrinsic-height gaps");

    // Enlarge beyond the viewport, then pan horizontally and vertically.
    vcx.simulate_event(scroll(20., true));
    vcx.run_until_parked();
    let before_drag = vcx.debug_bounds("pdf-page-0").unwrap();
    let dragged_to = position - point(px(30.), px(40.));
    vcx.simulate_mouse_down(position, gpui_kit::MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(dragged_to, gpui_kit::MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    let after_drag = vcx.debug_bounds("pdf-page-0").unwrap();
    assert!((after_drag.left() - before_drag.left() + px(30.)).abs() < px(1.), "drag pans horizontally");
    assert!((after_drag.top() - before_drag.top() + px(40.)).abs() < px(1.), "drag pans vertically");

    // Releasing outside the popup must stop panning.
    vcx.simulate_mouse_up(point(px(1.), px(1.)), gpui_kit::MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(position, gpui_kit::MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(vcx.debug_bounds("pdf-page-0").unwrap().origin, after_drag.origin);
}

#[gpui_kit::test]
async fn sorting_and_hidden_files(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("big.bin"), vec![0u8; 1000]).unwrap();
    fs::write(root.join("small.bin"), "x").unwrap();
    fs::write(root.join(".secret"), "").unwrap();

    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["big.bin", "small.bin"]).await;

    cx.update(|cx| {
        let pane = ws.read(cx).pane().clone();
        pane.update(cx, |p, cx| {
            p.set_sort(rile_core::SortKey::Size, cx);
            p.set_sort(rile_core::SortKey::Size, cx); // toggles to descending
        });
    });
    wait_for_names(cx, window, &ws, &["big.bin", "small.bin"]).await;
    cx.update(|cx| ws.read(cx).pane().clone().update(cx, |p, cx| p.set_sort(rile_core::SortKey::Size, cx)));
    wait_for_names(cx, window, &ws, &["small.bin", "big.bin"]).await;

    press(cx, window, &["cmd-shift-."]);
    wait_for_names(cx, window, &ws, &[".secret", "small.bin", "big.bin"]).await;
}

#[gpui_kit::test]
async fn views_quicklook_properties_and_typeahead(cx: &mut TestAppContext) {
    use rile_core::settings::ViewKind;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    for name in ["apple.txt", "banana.txt", "cherry.txt", "date.txt"] {
        fs::write(root.join(name), format!("{name}\nsecond line")).unwrap();
    }
    fs::create_dir(root.join("folder")).unwrap();
    fs::write(root.join("folder/inner.txt"), "x").unwrap();

    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["folder", "apple.txt", "banana.txt", "cherry.txt", "date.txt"]).await;
    let pane = cx.update(|cx| ws.read(cx).pane().clone());
    let cursor_name = |cx: &mut TestAppContext| cx.update(|cx| pane.read(cx).cursor_entry().map(|e| e.name.to_string()));

    // Type-ahead jumps to the first match.
    cx.simulate_input(window, "ch");
    cx.run_until_parked();
    assert_eq!(cursor_name(cx).as_deref(), Some("cherry.txt"));

    // Grid view: left/right move by one item.
    press(cx, window, &["cmd-2"]);
    assert_eq!(cx.update(|cx| pane.read(cx).view), ViewKind::Grid);
    press(cx, window, &["left"]);
    assert_eq!(cursor_name(cx).as_deref(), Some("banana.txt"));

    // Columns view: left goes to the parent folder.
    press(cx, window, &["cmd-3"]);
    assert_eq!(cx.update(|cx| pane.read(cx).view), ViewKind::Columns);
    press(cx, window, &["cmd-1"]);

    // Quick Look opens with space, loads a text preview, follows the cursor, closes with escape.
    press(cx, window, &["space"]);
    wait_until(cx, "quick look text", |cx| {
        let ws = ws.read(cx);
        ws.preview.quicklook
            && ws.preview.loaded.as_ref().is_some_and(|(p, _, lines)| p.ends_with("banana.txt") && lines.len() == 2)
    });
    press(cx, window, &["down"]);
    wait_until(cx, "preview follows cursor", |cx| {
        ws.read(cx).preview.loaded.as_ref().is_some_and(|(p, ..)| p.ends_with("cherry.txt"))
    });
    press(cx, window, &["escape"]);
    assert!(!cx.update(|cx| ws.read(cx).preview.quicklook));
    // Escape closed the preview but kept the selection.
    assert_eq!(cursor_name(cx).as_deref(), Some("cherry.txt"));

    // Properties dialog for a folder computes recursive totals.
    cx.update(|cx| pane.update(cx, |p, cx| p.cursor_to(0, false, cx)));
    // Properties open in the Quick Look card, not a dialog.
    press(cx, window, &["cmd-i"]);
    wait_until(cx, "properties preview", |cx| {
        let p = &ws.read(cx).preview;
        p.quicklook && p.properties_only && p.properties_view.is_some()
    });
}

#[gpui_kit::test]
async fn quicklook_markdown_edit_and_save(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("notes.md"), "# Title\n\n- item\n").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["notes.md"]).await;
    let pane = cx.update(|cx| ws.read(cx).pane().clone());
    cx.update(|cx| pane.update(cx, |p, cx| p.cursor_to(0, false, cx)));
    press(cx, window, &["space"]);
    wait_until(cx, "markdown preview", |cx| ws.read(cx).preview.loaded.as_ref().is_some_and(|(p, ..)| p.ends_with("notes.md")));

    window.update(cx, |_, w, cx| ws.update(cx, |ws, cx| ws.start_editing(w, cx))).unwrap();
    cx.run_until_parked();
    assert!(cx.update(|cx| ws.read(cx).preview.editor.is_some()));
    // Typing goes into the editor (not type-ahead), ⌘S writes the file.
    press(cx, window, &["cmd-down"]);
    cx.simulate_input(window, "more");
    press(cx, window, &["cmd-s"]);
    assert_eq!(fs::read_to_string(root.join("notes.md")).unwrap(), "# Title\n\n- item\nmore");
    assert!(cx.update(|cx| ws.read(cx).preview.quicklook));

    // Edits left unsaved are kept when Quick Look closes.
    cx.simulate_input(window, "!");
    window.update(cx, |_, _, cx| ws.update(cx, |ws, cx| ws.finish_editing(true, cx))).unwrap();
    cx.run_until_parked();
    assert!(cx.update(|cx| ws.read(cx).preview.editor.is_none()));
    assert!(fs::read_to_string(root.join("notes.md")).unwrap().ends_with("more!"));
}

#[gpui_kit::test]
async fn properties_footer_stays_visible_while_scrolling(cx: &mut TestAppContext) {
    use gpui_kit::{ScrollDelta, ScrollEvent, VisualTestContext, point, px};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("file.txt"), "properties fixture").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["file.txt"]).await;
    press(cx, window, &["down", "cmd-i"]);
    wait_until(cx, "properties preview", |cx| ws.read(cx).preview.quicklook);

    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let card = vcx.debug_bounds("quicklook-card").unwrap();
    let body = vcx.debug_bounds("properties-body").unwrap();
    let footer = vcx.debug_bounds("properties-footer").unwrap();
    assert!(footer.bottom() <= card.bottom(), "Copy Path must be visible before scrolling");
    assert!((footer.left() - card.left()).abs() <= px(1.), "footer must reach the left edge");
    assert!((footer.right() - card.right()).abs() <= px(1.), "footer must reach the right edge");
    assert!((footer.bottom() - card.bottom()).abs() <= px(1.), "footer must reach the bottom edge");
    assert!(body.bottom() <= footer.top(), "properties must not overlap the footer");
    assert!(vcx.debug_bounds("properties-footer-shadow").is_some());

    for lines in [-30., 30.] {
        vcx.simulate_event(ScrollEvent {
            position: body.center(),
            delta: ScrollDelta::Lines(point(0., lines)),
            ..Default::default()
        });
        vcx.run_until_parked();
        assert_eq!(vcx.debug_bounds("properties-footer").unwrap(), footer);
        let indicator = if lines < 0. { "properties-footer-no-shadow" } else { "properties-footer-shadow" };
        assert!(vcx.debug_bounds(indicator).is_some(), "shadow should indicate remaining content below");
    }
}

#[gpui_kit::test]
async fn filter_search_and_dual_pane(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::create_dir(root.join("dest")).unwrap();
    fs::write(root.join("a/b/needle.md"), "hay\nfind the NEEDLE here").unwrap();
    fs::write(root.join("a/other.md"), "nothing").unwrap();
    fs::write(root.join("top.txt"), "x").unwrap();

    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["a", "dest", "top.txt"]).await;

    // Typing a query leaves the listing intact until explicitly submitted.
    press(cx, window, &["cmd-f"]);
    cx.simulate_input(window, "to");
    cx.run_until_parked();
    wait_for_names(cx, window, &ws, &["a", "dest", "top.txt"]).await;
    cx.update(|cx| {
        let pane = ws.read(cx).pane().read(cx);
        assert!(pane.filter.is_empty());
        assert!(pane.search.is_none());
    });
    press(cx, window, &["enter"]);
    wait_for_names(cx, window, &ws, &["top.txt"]).await;

    // Recursive content search.
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            let mut q = ws.default_query(root.clone());
            q.content = Some(rile_services::search::ContentMatch { pattern: "needle".into(), regex: false, case_sensitive: false });
            ws.run_search(q, window, cx);
        })
    })
    .unwrap();
    wait_for_names(cx, window, &ws, &["needle.md"]).await;
    wait_until(cx, "search finished", |cx| ws.read(cx).pane().read(cx).search.as_ref().is_some_and(|s| !s.running));

    // Escape twice: clear selection, then leave search results.
    press(cx, window, &["escape", "escape"]);
    wait_for_names(cx, window, &ws, &["a", "dest", "top.txt"]).await;

    // Dual pane: other pane goes to "dest", F5 copies the selection there.
    press(cx, window, &["cmd-shift-d"]);
    assert!(cx.update(|cx| ws.read(cx).dual));
    let other = cx.update(|cx| ws.read(cx).panes[1].clone());
    cx.update(|cx| other.update(cx, |p, cx| p.navigate(Location::Dir(root.join("dest")), cx)));
    let first = cx.update(|cx| ws.read(cx).panes[0].clone());
    cx.update(|cx| first.update(cx, |p, cx| p.cursor_to(2, false, cx)));
    press(cx, window, &["f5"]);
    assert!(!root.join("dest/top.txt").exists());
    press(cx, window, &["escape"]);
    assert!(!root.join("dest/top.txt").exists());
    press(cx, window, &["f5", "enter"]);
    wait_until(cx, "copied to other pane", |_| root.join("dest/top.txt").exists());
    assert!(root.join("top.txt").exists());

    // Tab switches the active pane.
    press(cx, window, &["tab"]);
    assert_eq!(cx.update(|cx| ws.read(cx).active_pane), 1);

    // Dropping files onto a folder moves them (same filesystem).
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| ws.transfer(vec![root.join("top.txt")], root.join("a"), false, window, cx))
    })
    .unwrap();
    wait_until(cx, "moved by drop", |_| root.join("a/top.txt").exists() && !root.join("top.txt").exists());
}

#[gpui_kit::test]
async fn dual_pane_move_requires_confirmation(cx: &mut TestAppContext) {
    use gpui_kit::component::WindowExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let dest = root.join("dest");
    fs::create_dir(&dest).unwrap();
    fs::write(root.join("file.txt"), "keep me").unwrap();
    let (window, ws) = setup_with(&root, "update_check_on_start = false\nplugins_enabled = false\ndual_pane = true\n", None, cx);
    wait_for_names(cx, window, &ws, &["dest", "file.txt"]).await;
    let panes = cx.update(|cx| ws.read(cx).panes.clone());
    cx.update(|cx| {
        panes[1].update(cx, |p, cx| p.navigate(Location::Dir(dest.clone()), cx));
        panes[0].update(cx, |p, cx| p.cursor_to(1, false, cx));
    });
    press(cx, window, &["f6"]);
    assert!(cx.update_window(window, |_, window, cx| window.has_active_dialog(cx)).unwrap());
    assert!(root.join("file.txt").exists());
    assert!(!dest.join("file.txt").exists());
    press(cx, window, &["escape"]);
    assert!(root.join("file.txt").exists());
    press(cx, window, &["f6", "enter"]);
    wait_until(cx, "confirmed move", |_| dest.join("file.txt").exists() && !root.join("file.txt").exists());
}

#[gpui_kit::test]
async fn wasm_plugin_commands_and_preview(cx: &mut TestAppContext) {
    let wasm = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/hello/target/wasm32-wasip2/release/rile_plugin_hello.wasm");
    if !wasm.exists() {
        eprintln!("SKIPPED: build plugins/hello for wasm32-wasip2 first");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("data.csv"), "name,qty\napple,3\nbanana,12\n").unwrap();
    let (window, ws) = setup_with(&root, "update_check_on_start = false\nfavorites = []\n", Some(&wasm), cx);
    wait_for_names(cx, window, &ws, &["data.csv"]).await;
    wait_until(cx, "plugin loaded", |cx| !ws.read(cx).plugins.infos.is_empty());

    let commands = cx.update(|cx| ws.read(cx).plugin_commands(Some(root.clone()), vec![root.join("data.csv")]));
    let copy = commands.iter().find(|c| c.2 == "Copy file names").expect("plugin command listed").clone();

    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| ws.run_plugin_command(&copy.0, &copy.1, Some(root.clone()), vec![root.join("data.csv")], window, cx))
    })
    .unwrap();
    wait_until(cx, "clipboard from plugin", |cx| {
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref() == Some("data.csv")
    });

    // The plugin's CSV previewer feeds Quick Look.
    press(cx, window, &["down", "space"]);
    wait_until(cx, "plugin csv preview", |cx| {
        ws.read(cx).preview.loaded.as_ref().is_some_and(|(_, loaded, lines)| {
            matches!(**loaded, crate::workspace::preview::Loaded::Plugin(_)) && lines.iter().any(|l| l.contains("banana"))
        })
    });
}

#[gpui_kit::test]
async fn dual_pane_starts_with_left_active(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("f"), "").unwrap();
    let (window, ws) = setup_with(&root, "update_check_on_start = false\nplugins_enabled = false\ndual_pane = true\n", None, cx);
    wait_for_names(cx, window, &ws, &["f"]).await;
    let (active, flags) = cx.update(|cx| {
        let ws = ws.read(cx);
        (ws.active_pane, ws.panes.iter().map(|p| p.read(cx).active).collect::<Vec<_>>())
    });
    assert_eq!((active, flags), (0, vec![true, false]));
}

#[gpui_kit::test]
async fn backspace_navigates_only_the_active_pane(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let child = root.join("child");
    fs::create_dir(&child).unwrap();
    let (window, ws) = setup_with(&root, "update_check_on_start = false\nplugins_enabled = false\ndual_pane = true\n", None, cx);
    wait_for_names(cx, window, &ws, &["child"]).await;
    let panes = cx.update(|cx| ws.read(cx).panes.clone());
    cx.update(|cx| {
        for pane in &panes {
            pane.update(cx, |p, cx| p.navigate(Location::Dir(child.clone()), cx));
        }
    });
    press(cx, window, &["backspace"]);
    assert_eq!(cx.update(|cx| panes[0].read(cx).location.clone()), Location::Dir(root.clone()));
    assert_eq!(cx.update(|cx| panes[1].read(cx).location.clone()), Location::Dir(child.clone()));
    press(cx, window, &["tab", "backspace"]);
    assert_eq!(cx.update(|cx| panes[1].read(cx).location.clone()), Location::Dir(root.clone()));
    // Backspace in the filter edits text instead of navigating history.
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    press(cx, window, &[&format!("{modifier}-f"), "a", "b", "backspace"]);
    assert_eq!(cx.update(|cx| ws.read(cx).search_input.read(cx).value().to_string()), "a");
    assert_eq!(cx.update(|cx| panes[1].read(cx).location.clone()), Location::Dir(root));
}

#[gpui_kit::test]
async fn dual_pane_only_allows_list_view(cx: &mut TestAppContext) {
    use rile_core::settings::ViewKind;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &[]).await;
    press(cx, window, &["cmd-2", "cmd-shift-d"]);
    assert!(cx.update(|cx| ws.read(cx).all_panes().iter().all(|p| p.read(cx).view == ViewKind::List)));
    press(cx, window, &["cmd-2", "cmd-3", "tab", "cmd-2", "cmd-3"]);
    assert!(cx.update(|cx| ws.read(cx).all_panes().iter().all(|p| p.read(cx).view == ViewKind::List)));
    cx.update_window(window, |_, window, cx| ws.update(cx, |ws, cx| ws.open_tab(Location::Dir(root.clone()), window, cx))).unwrap();
    assert!(cx.update(|cx| ws.read(cx).all_panes().iter().all(|p| p.read(cx).view == ViewKind::List)));
    assert_eq!(cx.update(|cx| ws.read(cx).settings.default_view), ViewKind::Grid);
    press(cx, window, &["cmd-shift-d"]);
    assert_eq!(cx.update(|cx| ws.read(cx).pane().read(cx).view), ViewKind::Grid);
    press(cx, window, &["cmd-3"]);
    assert_eq!(cx.update(|cx| ws.read(cx).pane().read(cx).view), ViewKind::Columns);
}

#[gpui_kit::test]
async fn drive_selector_shortcuts_target_each_pane(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup_with(&root, "update_check_on_start = false\nplugins_enabled = false\ndual_pane = true\n", None, cx);
    wait_for_names(cx, window, &ws, &[]).await;
    let modifier = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    for (key, slot) in [("f2", 1), ("f1", 0)] {
        press(cx, window, &[&format!("{modifier}-{key}")]);
        assert_eq!(cx.update(|cx| ws.read(cx).drive_menu.as_ref().map(|(slot, _)| *slot)), Some(slot));
        assert_eq!(cx.update(|cx| ws.read(cx).active_pane), slot);
        press(cx, window, &["escape"]);
        assert!(cx.update(|cx| ws.read(cx).drive_menu.is_none()));
    }
    let drive = root.join("mounted-drive");
    {
        use gpui_kit::{Modifiers, MouseButton, VisualTestContext};
        let mut vcx = VisualTestContext::from_window(window, cx);
        vcx.run_until_parked();
        let button = vcx.debug_bounds("drive-selector-0").expect("left drive selector").center();
        vcx.simulate_mouse_down(button, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_up(button, MouseButton::Left, Modifiers::none());
        vcx.run_until_parked();
    }
    assert_eq!(cx.update(|cx| ws.read(cx).drive_menu.as_ref().map(|(slot, _)| *slot)), Some(0));
    press(cx, window, &["escape"]);
    fs::create_dir(&drive).unwrap();
    cx.update(|cx| ws.update(cx, |ws, cx| {
        ws.volumes = vec![rile_core::places::Volume {
            name: "Test drive".into(), mount_point: drive.clone(),
            total: 100, available: 50, removable: true,
        }];
        cx.notify();
    }));
    press(cx, window, &[&format!("{modifier}-f2"), "down", "enter"]);
    assert_eq!(cx.update(|cx| ws.read(cx).panes[1].read(cx).location.clone()), Location::Dir(drive));
    assert_eq!(cx.update(|cx| ws.read(cx).panes[0].read(cx).location.clone()), Location::Dir(root));
    assert!(cx.update(|cx| ws.read(cx).drive_menu.is_none()));
}

/// Regression: dialogs built inside a workspace action must not read the
/// workspace entity (gpui panics on re-entrant reads).
#[gpui_kit::test]
async fn dialogs_open_from_shortcuts(cx: &mut TestAppContext) {
    use gpui_kit::component::WindowExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("notes.txt"), "hello").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["notes.txt"]).await;
    let dialog_open = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.has_active_dialog(cx)).unwrap()
    };
    let close = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.close_all_dialogs(cx)).unwrap();
        cx.run_until_parked();
    };
    for keys in [&["cmd-,"][..], &["cmd-shift-f"], &["cmd-/"], &["down", "f2"]] {
        press(cx, window, keys);
        assert!(dialog_open(cx), "no dialog after {keys:?}");
        close(cx);
    }
    // Properties render in the Quick Look card instead of a dialog.
    press(cx, window, &["cmd-i"]);
    assert!(!dialog_open(cx));
    assert!(cx.update(|cx| ws.read(cx).preview.properties_only));
    // Settings and search form a second time (reuses the cached search form).
    press(cx, window, &["cmd-shift-f"]);
    assert!(dialog_open(cx));
}

/// Regression: dropping onto a folder in the same pane used to re-enter the
/// pane entity (resolving the drag selection) and abort the app.
#[gpui_kit::test]
async fn drag_file_onto_folder_in_same_pane(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, VisualTestContext};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "x").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["sub", "a.txt"]).await;

    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let from = vcx.debug_bounds("entry-a.txt").expect("a.txt row").center();
    let to = vcx.debug_bounds("entry-sub").expect("sub row").center();
    vcx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    for i in 1..=6 {
        let f = i as f32 / 6.;
        let p = gpui_kit::point(from.x + (to.x - from.x) * f, from.y + (to.y - from.y) * f);
        vcx.simulate_mouse_move(p, MouseButton::Left, Modifiers::none());
    }
    vcx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    wait_until(cx, "a.txt moved into sub", |_| root.join("sub/a.txt").exists() && !root.join("a.txt").exists());
}

/// Native payloads include directory metadata and the full selection, rather
/// than just the row under the mouse.
#[gpui_kit::test]
async fn external_drag_payload_includes_selected_files_and_folders(cx: &mut TestAppContext) {
    use crate::pane::DraggedFiles;
    use gpui_kit::{ExternalDragPayload, FileDragPaths};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), "x").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["sub", "a.txt"]).await;

    press(cx, window, &["cmd-a"]);
    cx.update(|cx| {
        let drag = DraggedFiles {
            source: ws.read(cx).pane().downgrade(),
            conn: None,
            path: root.join("a.txt"),
            label: "a.txt".into(),
        };
        assert_eq!(drag.external_payload(cx), Some(ExternalDragPayload::Files(FileDragPaths::new([
            (root.join("sub"), true), (root.join("a.txt"), false),
        ]))));
    });
    assert!(root.join("a.txt").exists(), "handoff must not delete source files");
}

#[gpui_kit::test]
async fn external_drag_payload_only_exports_local_dragged_entries(cx: &mut TestAppContext) {
    use crate::pane::DraggedFiles;
    use gpui_kit::{ExternalDragPayload, FileDragPaths};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("a.txt"), "a").unwrap();
    fs::write(root.join("b.txt"), "b").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["a.txt", "b.txt"]).await;
    press(cx, window, &["home"]);
    cx.update(|cx| {
        let source = ws.read(cx).pane().downgrade();
        let mut drag = DraggedFiles { source, conn: None, path: root.join("b.txt"), label: "b.txt".into() };
        let expected = Some(ExternalDragPayload::Files(FileDragPaths::new([(root.join("b.txt"), false)])));
        assert_eq!(drag.external_payload(cx), expected, "unselected row must not export another selection");
        drag.path = root.join("a.txt");
        assert_eq!(drag.external_payload(cx), Some(ExternalDragPayload::Files(FileDragPaths::new([(root.join("a.txt"), false)]))));
        drag.conn = Some("remote".into());
        assert_eq!(drag.external_payload(cx), None, "remote paths are not local files");
        drag.conn = None;
        drag.path = root.join("missing.txt");
        assert_eq!(drag.external_payload(cx), None, "stale rows must not export an empty payload");
    });
}

#[gpui_kit::test]
async fn rubber_band_new_file_and_mouse_navigation(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, NavigationDirection, VisualTestContext, point, px};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join("dir")).unwrap();
    for f in ["a.txt", "b.txt", "c.txt"] {
        fs::write(root.join(f), "").unwrap();
    }
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["dir", "a.txt", "b.txt", "c.txt"]).await;
    let pane = cx.update(|cx| ws.read(cx).pane().clone());

    // Drag from empty space below the rows up to "a.txt": selects a, b, c.
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let a = vcx.debug_bounds("entry-a.txt").unwrap();
    let c = vcx.debug_bounds("entry-c.txt").unwrap();
    let start = point(c.center().x, c.bottom() + px(60.));
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(point(start.x, a.center().y), MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    assert!(vcx.update(|_, cx| pane.read(cx).rubber.is_some()), "band visible while dragging");
    vcx.simulate_mouse_up(point(start.x, a.center().y), MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    let mut names: Vec<String> = vcx.update(|_, cx| {
        pane.read(cx).selected_entries().iter().map(|e| e.name.to_string()).collect()
    });
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
    assert!(vcx.update(|_, cx| pane.read(cx).rubber.is_none()));

    // New file via shortcut, named through the rename dialog.
    press(cx, window, &["cmd-alt-n"]);
    wait_for_names(cx, window, &ws, &["dir", "a.txt", "b.txt", "c.txt", "untitled.txt"]).await;
    cx.simulate_input(window, "notes.md");
    press(cx, window, &["enter"]);
    wait_for_names(cx, window, &ws, &["dir", "a.txt", "b.txt", "c.txt", "notes.md"]).await;

    // Mouse thumb buttons: into "dir" then back and forward.
    cx.update(|cx| pane.update(cx, |p, cx| p.navigate(Location::Dir(root.join("dir")), cx)));
    wait_for_names(cx, window, &ws, &[]).await;
    let mut vcx = VisualTestContext::from_window(window, cx);
    let center = vcx.update(|window, _| {
        let s = window.viewport_size();
        point(s.width * 0.6, s.height * 0.5)
    });
    vcx.simulate_mouse_down(center, MouseButton::Navigate(NavigationDirection::Back), Modifiers::none());
    vcx.run_until_parked();
    wait_for_names(cx, window, &ws, &["dir", "a.txt", "b.txt", "c.txt", "notes.md"]).await;
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.simulate_mouse_down(center, MouseButton::Navigate(NavigationDirection::Forward), Modifiers::none());
    vcx.run_until_parked();
    wait_for_names(cx, window, &ws, &[]).await;
}

mod ftp_server {
    use std::sync::Arc;

    use async_trait::async_trait;
    use unftp_core::auth::{AuthenticationError, Authenticator, Credentials, Principal};
    use unftp_sbe_fs::Filesystem;

    pub const USER: &str = "alice";
    pub const PASS: &str = "s3cret";

    #[derive(Debug)]
    struct PasswordAuth;

    #[async_trait]
    impl Authenticator for PasswordAuth {
        async fn authenticate(&self, user: &str, creds: &Credentials) -> Result<Principal, AuthenticationError> {
            if user == USER && creds.password.as_deref() == Some(PASS) {
                Ok(Principal { username: user.into() })
            } else {
                Err(AuthenticationError::BadPassword)
            }
        }
    }

    /// Starts an FTP server serving `root` on a free localhost port.
    pub fn start(root: std::path::PathBuf) -> u16 {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            rt.block_on(async move {
                let server = libunftp::ServerBuilder::with_authenticator(
                    Box::new(move || Filesystem::new(root.clone()).unwrap()),
                    Arc::new(PasswordAuth),
                )
                .passive_ports(40000..=49999)
                .build()
                .unwrap();
                server.listen(format!("127.0.0.1:{port}")).await.unwrap();
            });
        });
        for _ in 0..200 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return port;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("ftp server didn't start");
    }
}

/// End to end over FTP: connect, browse, download, upload, rename, delete,
/// and Quick Look of a remote file.
#[gpui_kit::test]
async fn ftp_browse_transfer_and_edit(cx: &mut TestAppContext) {
    let server = tempfile::tempdir().unwrap();
    let server_root = server.path().canonicalize().unwrap();
    fs::create_dir(server_root.join("docs")).unwrap();
    fs::write(server_root.join("hello.txt"), "hello from ftp\nline 2").unwrap();
    let port = ftp_server::start(server_root.clone());

    let local = tempfile::tempdir().unwrap();
    let local_root = local.path().canonicalize().unwrap();
    fs::write(local_root.join("up.txt"), "uploaded").unwrap();

    let (window, ws) = setup(&local_root, cx);
    wait_for_names(cx, window, &ws, &["up.txt"]).await;

    let info = rile_remote::ConnectionInfo {
        id: rile_remote::ConnectionInfo::new_id(),
        name: "Test FTP".into(),
        protocol: rile_remote::Protocol::Ftp,
        host: "127.0.0.1".into(),
        port,
        username: ftp_server::USER.into(),
        auth: rile_remote::Auth::Password,
        initial_path: None,
    };
    let conn = info.id.clone();
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| ws.connect(info, Some(ftp_server::PASS.into()), false, window, cx))
    })
    .unwrap();
    wait_for_names(cx, window, &ws, &["docs", "hello.txt"]).await;
    let pane = cx.update(|cx| ws.read(cx).pane().clone());
    assert_eq!(cx.update(|cx| pane.read(cx).conn()), Some(conn.clone()));

    // Quick Look streams the head of the remote file.
    cx.update(|cx| pane.update(cx, |p, cx| p.cursor_to(1, false, cx)));
    press(cx, window, &["space"]);
    wait_until(cx, "remote preview", |cx| {
        ws.read(cx).preview.loaded.as_ref().is_some_and(|(_, _, lines)| lines.first().is_some_and(|l| l == "hello from ftp"))
    });
    press(cx, window, &["escape"]);

    // Download hello.txt into the local folder.
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.transfer_between(vec!["/hello.txt".into()], Some(conn.clone()), local_root.clone(), None, true, window, cx)
        })
    })
    .unwrap();
    wait_until(cx, "download", |_| fs::read_to_string(local_root.join("hello.txt")).is_ok_and(|s| s.starts_with("hello from ftp")));

    // Upload up.txt into /docs, then open docs and see it.
    cx.update_window(window, |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.transfer_between(vec![local_root.join("up.txt")], None, "/docs".into(), Some(conn.clone()), true, window, cx)
        })
    })
    .unwrap();
    wait_until(cx, "upload", |_| server_root.join("docs/up.txt").exists());
    cx.update(|cx| pane.update(cx, |p, cx| p.cursor_to(0, false, cx)));
    press(cx, window, &["enter"]);
    wait_for_names(cx, window, &ws, &["up.txt"]).await;

    // Rename on the server through the rename dialog.
    cx.update(|cx| pane.update(cx, |p, cx| p.cursor_to(0, false, cx)));
    press(cx, window, &["f2"]);
    cx.simulate_input(window, "renamed.txt");
    press(cx, window, &["enter"]);
    wait_until(cx, "remote rename", |_| server_root.join("docs/renamed.txt").exists());
    wait_for_names(cx, window, &ws, &["renamed.txt"]).await;

    // Delete (servers have no trash: always confirms).
    press(cx, window, &["cmd-backspace", "enter"]);
    wait_until(cx, "remote delete", |_| !server_root.join("docs/renamed.txt").exists());
    wait_for_names(cx, window, &ws, &[]).await;

    // Back up to the server root.
    press(cx, window, &["cmd-up"]);

    wait_for_names(cx, window, &ws, &["docs", "hello.txt"]).await;
}

/// Tabs: ⌘T opens the Overview in a new tab with its own history, ⌃Tab
/// cycles, ⌘W closes, and closing the last tab returns it to the Overview.
#[gpui_kit::test]
async fn tabs_open_switch_and_close(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/inner.txt"), "").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["sub"]).await;
    let location = |cx: &mut TestAppContext| cx.update(|cx| ws.read(cx).pane().read(cx).location.clone());
    let tabs = |cx: &mut TestAppContext| cx.update(|cx| (ws.read(cx).slot_tabs().len(), ws.read(cx).active_tab()));

    press(cx, window, &["cmd-t"]);
    assert_eq!(tabs(cx), (2, 1));
    assert_eq!(location(cx), Location::Home);
    // The new tab navigates on its own; the first keeps its folder.
    cx.update(|cx| ws.update(cx, |ws, cx| ws.pane().update(cx, |p, cx| p.navigate(Location::Dir(root.join("sub")), cx))));
    wait_for_names(cx, window, &ws, &["inner.txt"]).await;
    press(cx, window, &["ctrl-tab"]);
    assert_eq!(tabs(cx), (2, 0));
    assert_eq!(location(cx), Location::Dir(root.clone()));
    assert!(!cx.update(|cx| ws.read(cx).pane().read(cx).history.can_go_back()));

    // Close the first tab: the remaining one (inside "sub") becomes active.
    press(cx, window, &["cmd-w"]);
    assert_eq!(tabs(cx), (1, 0));
    assert_eq!(location(cx), Location::Dir(root.join("sub")));
    // The last tab can't close; it goes back to the Overview instead.
    press(cx, window, &["cmd-w"]);
    assert_eq!(tabs(cx), (1, 0));
    assert_eq!(location(cx), Location::Home);

    // ⇧⌘O opens the selected folder in a new tab.
    cx.update(|cx| ws.update(cx, |ws, cx| ws.pane().update(cx, |p, cx| p.navigate(Location::Dir(root.clone()), cx))));
    wait_for_names(cx, window, &ws, &["sub"]).await;
    press(cx, window, &["down", "cmd-shift-o"]);
    assert_eq!(tabs(cx), (2, 1));
    wait_for_names(cx, window, &ws, &["inner.txt"]).await;
}

/// The header toggle opens a second pane and either pane can close it.
#[gpui_kit::test]
async fn dual_pane_header_toggle(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, VisualTestContext};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &[]).await;

    let mut vcx = VisualTestContext::from_window(window, cx);
    for (selector, expected) in [("dual-pane-toggle-0", true), ("dual-pane-toggle-1", false)] {
        vcx.run_until_parked();
        let toggle = vcx.debug_bounds(selector).expect("dual-pane header toggle").center();
        vcx.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::none());
        vcx.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::none());
        vcx.run_until_parked();
        assert_eq!(cx.update(|cx| ws.read(cx).dual), expected);
    }
}

/// Rapid keyboard toggles reverse the slide and settle at the requested width.
#[gpui_kit::test]
async fn sidebar_animation_reverses_and_finishes(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &[]).await;
    let width = cx.update_window(window, |_, window, cx| ws.read(cx).sidebar_display_width(window)).unwrap();

    press(cx, window, &["cmd-shift-s", "cmd-shift-s"]);
    assert!(cx.update(|cx| ws.read(cx).sidebar_visible));
    std::thread::sleep(Duration::from_millis(250));
    let restored = cx.update_window(window, |_, window, cx| ws.read(cx).sidebar_display_width(window)).unwrap();
    assert_eq!(restored, width);

    press(cx, window, &["cmd-shift-s"]);
    std::thread::sleep(Duration::from_millis(250));
    let collapsed = cx.update_window(window, |_, window, cx| ws.read(cx).sidebar_display_width(window)).unwrap();
    assert_eq!(collapsed, 0.);
}

/// The sidebar header toggle hides it; the toolbar toggle brings it back.
#[gpui_kit::test]
async fn sidebar_toggle_button_moves_with_visibility(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, VisualTestContext};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &[]).await;

    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let toggle = vcx.debug_bounds("sidebar-toggle").expect("sidebar toggle").center();
    let header = vcx.debug_bounds("sidebar-header").expect("sidebar header");
    let rail = cx.update_window(window, |_, window, _| {
        crate::views::chrome::width_class(window) == crate::views::chrome::Width::Phone
    }).unwrap();
    if !rail {
        assert!(header.contains(&toggle), "toggle should be beside the logo in the sidebar header");
    }
    vcx.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    assert!(!cx.update(|cx| ws.read(cx).sidebar_visible));

    let toggle = vcx.debug_bounds("sidebar-toggle").expect("toolbar restore toggle").center();
    vcx.simulate_mouse_down(toggle, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_up(toggle, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    assert!(cx.update(|cx| ws.read(cx).sidebar_visible));
}

/// The inspector loads real metadata (properties + image dimensions) for
/// the selection, and the sidebar toggles with ⇧⌘S.
#[gpui_kit::test]
async fn inspector_metadata_and_sidebar_toggle(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    // A 3×2 PNG (header is all imagesize needs).
    let png: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, 0x49, 0x48, 0x44, 0x52, 0, 0, 0, 3, 0, 0, 0, 2, 8, 6, 0, 0, 0,
    ];
    fs::write(root.join("pic.png"), png).unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["pic.png"]).await;
    assert!(cx.update(|cx| ws.read(cx).settings.show_preview_panel), "inspector is on by default");
    press(cx, window, &["down"]);
    wait_until(cx, "inspector info", |cx| ws.read(cx).preview.info.is_some());
    let (dims, has_props) = cx.update(|cx| {
        let (_, info) = ws.read(cx).preview.info.clone().unwrap();
        (info.dimensions, info.props.is_some())
    });
    assert_eq!(dims, Some((3, 2)));
    assert!(has_props);

    press(cx, window, &["cmd-shift-s"]);
    assert!(!cx.update(|cx| ws.read(cx).sidebar_visible));
    press(cx, window, &["cmd-shift-s"]);
    assert!(cx.update(|cx| ws.read(cx).sidebar_visible));

    // Favorites (folders) moved to ⌃⌘T (⌘T is New Tab); with a file
    // selected, the current folder is added.
    press(cx, window, &["cmd-ctrl-t"]);
    assert!(cx.update(|cx| ws.read(cx).settings.favorites.contains(&root)));
}

/// Overview statistics come from the top level of the places, with
/// modification-date activity for the last seven days.
#[test]
fn overview_stats_from_places() {
    use rile_core::places::{Place, PlaceIcon};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join("folder")).unwrap();
    for name in ["a.png", "b.rs", "c.pdf", "d.zip", "e.bin"] {
        fs::write(root.join(name), "x").unwrap();
    }
    let places = vec![
        Place { name: "Docs".into(), path: root.clone(), icon: PlaceIcon::Documents },
        // Home is skipped (its top level is mostly the other places).
        Place { name: "Home".into(), path: root.clone(), icon: PlaceIcon::Home },
    ];
    let stats = crate::views::home::compute(&places, std::time::SystemTime::now());
    assert_eq!((stats.items, stats.files, stats.folders, stats.places), (6, 5, 1, 1));
    let counts: Vec<usize> = stats.groups.iter().map(|g| g.count).collect();
    assert_eq!(counts, [1, 1, 1, 0, 1, 1]);
    // Everything was just created: all activity is today.
    assert_eq!(stats.activity[6], 6);
    assert_eq!(stats.activity[..6].iter().sum::<usize>(), 0);
}

/// Regression: previews were cached by path alone, so the same path on a
/// different filesystem (local vs a server, or two servers) reused stale
/// content and metadata.
#[gpui_kit::test]
async fn preview_cache_is_keyed_by_connection(cx: &mut TestAppContext) {
    use crate::workspace::preview::Loaded;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("notes.txt"), "local text").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["notes.txt"]).await;
    // Pretend the same path was last previewed on a server.
    cx.update(|cx| {
        ws.update(cx, |ws, _| {
            let path = root.join("notes.txt");
            ws.preview.target = Some(path.clone());
            ws.preview.target_conn = Some("server".into());
            ws.preview.loaded = Some((path, std::sync::Arc::new(Loaded::Error("stale".into())), Default::default()));
            ws.preview.loaded_conn = Some("server".into());
        })
    });
    press(cx, window, &["down"]);
    wait_until(cx, "local preview", |cx| {
        let p = &ws.read(cx).preview;
        p.loaded_conn.is_none() && p.loaded.as_ref().is_some_and(|(_, l, _)| !matches!(**l, Loaded::Error(_)))
    });
    assert!(cx.update(|cx| ws.read(cx).preview.info.is_some()));
}

/// Regression: switching panes left the shared search field showing the
/// other pane's filter.
#[gpui_kit::test]
async fn search_field_follows_active_pane(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    for n in ["apple", "banana"] {
        fs::write(root.join(n), "").unwrap();
    }
    let (window, ws) = setup_with(&root, "update_check_on_start = false\nplugins_enabled = false\ndual_pane = true\n", None, cx);
    wait_for_names(cx, window, &ws, &["apple", "banana"]).await;
    let field = |cx: &mut TestAppContext| cx.update(|cx| ws.read(cx).search_input.read(cx).value().to_string());
    press(cx, window, &["cmd-f"]);
    cx.simulate_input(window, "app");
    cx.run_until_parked();
    assert!(cx.update(|cx| ws.read(cx).pane().read(cx).filter.is_empty()));
    // Existing pane filters still synchronize independently of query typing.
    cx.update(|cx| ws.read(cx).pane().clone().update(cx, |p, cx| p.set_filter("app".into(), cx)));
    cx.update_window(window, |_, window, cx| ws.update(cx, |ws, cx| ws.set_active_pane(1, window, cx))).unwrap();
    cx.run_until_parked();
    assert_eq!(field(cx), "");
    cx.update_window(window, |_, window, cx| ws.update(cx, |ws, cx| ws.set_active_pane(0, window, cx))).unwrap();
    cx.run_until_parked();
    assert_eq!(field(cx), "app");
}

/// Thumbnails are decoded downscaled (not kept at full resolution).
#[test]
fn thumbnails_are_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("big.png");
    image::RgbaImage::from_pixel(2400, 1200, image::Rgba([10, 20, 30, 255])).save(&path).unwrap();
    let thumb = crate::art::decode_thumbnail(&path, 256).expect("decodes");
    let size = thumb.size(0);
    assert_eq!((size.width.0, size.height.0), (256, 128));
    // Stored as BGRA for gpui.
    assert_eq!(&thumb.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
}

/// Regression (P1): Overview and folder headers used window breakpoints, so
/// a dual-pane slot beside the sidebar and inspector got the four-column
/// layout and collapsed. Layout now follows the slot's own width.
#[gpui_kit::test]
async fn overview_layout_follows_slot_width(cx: &mut TestAppContext) {
    use crate::views::home::overview_layout;
    assert_eq!(overview_layout(1300.).metric_cols, 5);
    assert_eq!(overview_layout(1100.).metric_cols, 3);
    assert_eq!(overview_layout(580.).metric_cols, 2);
    assert_eq!(overview_layout(380.).metric_cols, 1);
    assert!(overview_layout(580.).stacked && !overview_layout(1100.).stacked);
    assert_eq!((overview_layout(580.).storage_cols, overview_layout(700.).storage_cols), (1, 2));

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("f"), "").unwrap();
    let (window, ws) = setup_with(&root, "update_check_on_start = false\nplugins_enabled = false\ndual_pane = true\n", None, cx);
    wait_for_names(cx, window, &ws, &["f"]).await;
    press(cx, window, &["cmd-t"]);
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        let ws = ws.read(cx);
        let viewport: f32 = window.viewport_size().width.into();
        let slot = ws.slot_width(0, window);
        let chrome = crate::sidebar::width(ws, window) + if ws.inspector_inline(window) { crate::workspace::inspector_width(window) } else { 0. };
        assert!(slot <= (viewport - chrome) / 2. + 2., "slot {slot} of {viewport}");
        assert!(overview_layout(slot).metric_cols <= overview_layout(viewport).metric_cols);
        if viewport < 1500. {
            assert!(overview_layout(slot).metric_cols <= 2, "slot {slot} of {viewport}");
        }
    })
    .unwrap();
}

/// Glass is limited to native chrome. In-window surfaces must not expose
/// file rows beneath notifications, menus, dialogs or floating panels.
#[gpui_kit::test]
async fn glass_overlay_surfaces_stay_opaque_in_both_themes(cx: &mut TestAppContext) {
    use rile_core::settings::ThemeMode;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup(&root, cx);
    for (mode, glass) in [
        (ThemeMode::Dark, true), (ThemeMode::Light, true),
        (ThemeMode::Dark, false), (ThemeMode::Light, false), (ThemeMode::Dark, true),
    ] {
        cx.update_window(window, |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.settings.theme = mode;
                ws.settings.glass = glass;
                ws.settings_changed(window, cx);
            });
            let ch = crate::theme::chrome(cx);
            let (top, bottom) = ch.surface_colors();
            assert_eq!(top.a, 1., "raised surface must be opaque: {mode:?}, glass={glass}");
            assert_eq!(bottom.a, 1.);
            assert_eq!(ch.inspector.a, 1.);
            assert_eq!(ch.statusbar.a, 1.);
            assert_eq!(ch.browser.a, 1.);
            assert_eq!(gpui_kit::component::Theme::global(cx).colors.popover.a, 1.);
            if glass {
                assert_eq!(ch.window.a, 0.);
                assert!(ch.sidebar_top.a < 1.);
                assert!(ch.toolbar_top.a < 1.);
            }
        }).unwrap();
    }
}

#[gpui_kit::test]
async fn glass_tint_and_translucency_apply_only_to_chrome(cx: &mut TestAppContext) {
    use rile_core::settings::ThemeMode;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup(&root, cx);
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        for translucency in [0, 50, 100, 255] {
            cx.update_window(window, |_, window, cx| {
                ws.update(cx, |ws, cx| {
                    ws.settings.theme = mode;
                    ws.settings.glass = true;
                    ws.settings.glass_hue = 210;
                    ws.settings.glass_translucency = translucency;
                    ws.settings_changed(window, cx);
                });
                let ch = crate::theme::chrome(cx);
                let alpha = 1. - translucency.min(100) as f32 / 100.;
                for tint in [ch.sidebar_top, ch.sidebar_bottom, ch.toolbar_top, ch.toolbar_bottom, ch.tabstrip] {
                    assert!((tint.h - 210. / 360.).abs() < 0.001);
                    assert!((tint.a - alpha).abs() < 0.001);
                }
                assert_eq!(ch.surface_colors().0.a, 1.);
                assert_eq!(ch.browser.a, 1.);
                ws.update(cx, |ws, cx| {
                    ws.settings.glass = false;
                    ws.settings_changed(window, cx);
                });
                let solid = crate::theme::chrome(cx);
                assert_eq!(solid.sidebar_top.a, 1.);
                assert_eq!(solid.toolbar_top.a, 1.);
            }).unwrap();
        }
    }
}

/// Unlocks render above any location without navigating away or stealing
/// keyboard focus, and survive ordinary workspace re-renders.
#[gpui_kit::test]
async fn trophy_unlock_animation_renders_without_changing_location(cx: &mut TestAppContext) {
    use gpui_kit::VisualTestContext;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("keep.txt"), "x").unwrap();
    let (window, ws) = setup_with(&root, "update_check_on_start = false\nplugins_enabled = false\nglass = true\n", None, cx);
    wait_for_names(cx, window, &ws, &["keep.txt"]).await;
    cx.update(|cx| ws.update(cx, |ws, cx| {
        ws.rewards.enqueue([rile_services::rewards::Unlock {
            id: "searches:10".into(), name: "Search detective".into(), detail: "10 searches completed".into(),
        }]);
        cx.notify();
    }));
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.run_until_parked();
    let bounds = vcx.debug_bounds("reward-celebration").expect("unlock animation is visible");
    assert!(f32::from(bounds.size.width) > 0.);
    assert!(f32::from(bounds.size.height) > 0.);
    let viewport = vcx.update(|window, _| window.viewport_size());
    let right_gap = f32::from(viewport.width - bounds.origin.x - bounds.size.width);
    let bottom_gap = f32::from(viewport.height - bounds.origin.y - bounds.size.height);
    assert!((16. ..=20.).contains(&right_gap), "notification is aligned right: {right_gap}");
    assert!((31. ..=64.).contains(&bottom_gap), "notification stays above the status bar: {bottom_gap}");
    assert!(f32::from(bounds.origin.y) > f32::from(viewport.height) / 2.);
    vcx.update(|_, cx| {
        assert_eq!(ws.read(cx).pane().read(cx).location, Location::Dir(root));
        ws.update(cx, |_, cx| cx.notify());
    });
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("reward-celebration").is_some());
}

/// The toolbar clears the macOS traffic lights whenever the visible
/// sidebar is narrower than they are (hidden sidebar or the icon rail).
#[test]
fn toolbar_clears_traffic_lights() {
    use crate::views::chrome::{TRAFFIC_LIGHTS_END, toolbar_inset};
    assert_eq!(toolbar_inset(14., 220., true), 14.);
    assert_eq!(toolbar_inset(9., 60., true), TRAFFIC_LIGHTS_END - 60.);
    assert_eq!(toolbar_inset(14., 0., true), TRAFFIC_LIGHTS_END);
    assert_eq!(toolbar_inset(9., 0., false), 9.);
}

/// Sidebar sections reorder (as a drag of one header onto another section
/// does) and the order is saved.
#[gpui_kit::test]
async fn sidebar_sections_reorder_and_persist(cx: &mut TestAppContext) {
    use rile_core::settings::SidebarSection::*;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("f"), "").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["f"]).await;
    let _config = config_lock(); // keep RILE_CONFIG_DIR ours until the reload
    cx.update_window(window, |_, window, cx| ws.update(cx, |ws, cx| ws.move_sidebar_section(Favorites, Locations, window, cx)))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| ws.read(cx).settings.sidebar_order()), [Library, Locations, Favorites, Devices, Network]);
    let saved = rile_core::settings::Settings::load();
    assert_eq!(saved.sidebar_order(), [Library, Locations, Favorites, Devices, Network]);
}

/// Folders the user opens are remembered, and a later search from a parent
/// answers from that memory (verified on disk) before walking.
#[gpui_kit::test]
async fn opened_folders_are_remembered_for_search(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("sub/deep")).unwrap();
    fs::write(root.join("sub/deep/needle-report.txt"), "").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["sub"]).await;
    let open = |cx: &mut TestAppContext, dir: std::path::PathBuf| {
        cx.update(|cx| ws.update(cx, |ws, cx| ws.pane().update(cx, |p, cx| p.navigate(Location::Dir(dir), cx))))
    };
    open(cx, root.join("sub/deep"));
    wait_for_names(cx, window, &ws, &["needle-report.txt"]).await;

    let mut q = rile_services::search::SearchQuery::new(&root);
    q.name = rile_services::search::NameMatch::Substring("needle".into());
    let hits: Vec<_> = rile_services::index::query(&q).into_iter().map(|e| e.path).collect();
    assert_eq!(hits, [root.join("sub/deep/needle-report.txt")]);

    // The search shows it once (index hit + walk hit are merged).
    open(cx, root.clone());
    wait_for_names(cx, window, &ws, &["sub"]).await;
    cx.update_window(window, |_, window, cx| ws.update(cx, |ws, cx| ws.run_search(q.clone(), window, cx))).unwrap();
    wait_until(cx, "search done", |cx| ws.read(cx).pane().read(cx).search.as_ref().is_some_and(|s| !s.running));
    let names: Vec<String> = cx.update(|cx| ws.read(cx).pane().read(cx).entries().iter().map(|e| e.name.to_string()).collect());
    assert_eq!(names, ["needle-report.txt"]);
}

/// Intelligent search shares the main field; empty requests and ⌘K
/// focus it without opening a dialog.
#[gpui_kit::test]
async fn intelligent_search_uses_main_field(cx: &mut TestAppContext) {
    use gpui_kit::component::WindowExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("report.txt"), "").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["report.txt"]).await;

    press(cx, window, &["cmd-k", "shift-enter"]);
    cx.update_window(window, |_, window, cx| {
        assert!(!window.has_active_dialog(cx));
        let input = ws.read(cx).search_input.clone();
        assert!(gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window));
        assert!(ws.read(cx).pane().read(cx).search.is_none());
    }).unwrap();

    // Disabled AI must not silently run a normal recursive search.
    cx.update(|cx| ws.update(cx, |ws, _| ws.settings.ai_enabled = false));
    cx.simulate_input(window, "report");
    press(cx, window, &["shift-enter"]);
    cx.update(|cx| {
        assert!(ws.read(cx).pane().read(cx).search.is_none());
        assert_eq!(ws.read(cx).search_input.read(cx).value().as_ref(), "report");
    });
    press(cx, window, &["enter"]);
    wait_until(cx, "regular search completes", |cx| {
        ws.read(cx).pane().read(cx).search.as_ref().is_some_and(|s| !s.running)
    });
    wait_for_names(cx, window, &ws, &["report.txt"]).await;
}

/// With no model compiled in, errors stay inline in the search results,
/// rather than opening the assistant or running a literal-name search.
#[cfg(not(feature = "ai"))]
#[gpui_kit::test]
async fn intelligent_search_reports_errors_inline(cx: &mut TestAppContext) {
    use gpui_kit::component::WindowExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (window, ws) = setup(&root, cx);
    press(cx, window, &["cmd-f"]);
    cx.simulate_input(window, "  invoices from last month  ");
    press(cx, window, &["shift-enter"]);
    wait_until(cx, "AI error", |cx| {
        ws.read(cx).pane().read(cx).search.as_ref().is_some_and(|s| !s.running && s.error.is_some())
    });
    cx.update(|cx| {
        let pane = ws.read(cx).pane().read(cx);
        assert_eq!(pane.search.as_ref().unwrap().label.as_ref(), "AI: \"invoices from last month\"");
        assert!(ws.read(cx).search_input.read(cx).value().is_empty());
    });
    cx.update_window(window, |_, window, cx| assert!(!window.has_active_dialog(cx))).unwrap();
}

/// ⌘F focuses the search field on the Overview too, without filtering
/// its results while the user is still composing a query.
#[gpui_kit::test]
async fn cmd_f_focuses_search_on_overview(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("f"), "").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["f"]).await;
    press(cx, window, &["cmd-shift-h"]);
    assert_eq!(cx.update(|cx| ws.read(cx).pane().read(cx).location.clone()), Location::Home);
    press(cx, window, &["cmd-f"]);
    let focused = cx
        .update_window(window, |_, window, cx| {
            let input = ws.read(cx).search_input.clone();
            gpui_kit::Focusable::focus_handle(input.read(cx), cx).is_focused(window)
        })
        .unwrap();
    assert!(focused, "search field not focused on the Overview");
    cx.simulate_input(window, "report");
    cx.run_until_parked();
    assert!(cx.update(|cx| ws.read(cx).pane().read(cx).filter.is_empty()));
    assert_eq!(cx.update(|cx| ws.read(cx).search_input.read(cx).value().to_string()), "report");
}

/// Blank clicks deselect results without dismissing the search, even
/// when nothing was selected before the click.
#[gpui_kit::test]
async fn blank_click_preserves_search_results(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, VisualTestContext, point, px};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/found.txt"), "").unwrap();
    fs::write(root.join("other.txt"), "").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["sub", "other.txt"]).await;

    for view_key in ["cmd-1", "cmd-2"] {
        press(cx, window, &[view_key, "cmd-f"]);
        cx.simulate_input(window, "found");
        press(cx, window, &["enter"]);
        wait_until(cx, "search finished", |cx| {
            ws.read(cx).pane().read(cx).search.as_ref().is_some_and(|s| !s.running)
        });
        wait_for_names(cx, window, &ws, &["found.txt"]).await;
        let pane = cx.update(|cx| ws.read(cx).pane().clone());
        cx.update(|cx| pane.update(cx, |p, cx| p.cursor_to(0, false, cx)));
        let mut vcx = VisualTestContext::from_window(window, cx);
        vcx.run_until_parked();
        let row = vcx.debug_bounds("entry-found.txt").unwrap();
        let blank = point(row.center().x, row.bottom() + px(60.));
        for _ in 0..2 {
            vcx.simulate_mouse_down(blank, MouseButton::Left, Modifiers::none());
            vcx.simulate_mouse_up(blank, MouseButton::Left, Modifiers::none());
            vcx.run_until_parked();
            vcx.update(|_, cx| {
                let pane = pane.read(cx);
                assert!(pane.selected.is_empty());
                assert!(pane.search.is_some(), "blank click dismissed the search in {view_key}");
                assert_eq!(pane.entries().len(), 1);
                assert_eq!(pane.entries()[0].name.as_ref(), "found.txt");
            });
        }
    }
}

/// Esc stops a running search but keeps what it found; a second Esc
/// clears the search. (The test scheduler runs background work on this
/// thread, so the worker can't block; Esc is dispatched before it runs.)
#[gpui_kit::test]
async fn escape_stops_search_and_keeps_results(cx: &mut TestAppContext) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("found.txt"), "").unwrap();
    fs::write(root.join("late.txt"), "").unwrap();
    let (window, ws) = setup(&root, cx);
    wait_for_names(cx, window, &ws, &["found.txt", "late.txt"]).await;
    let state = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            let pane = ws.read(cx).pane().read(cx);
            let names: Vec<String> = pane.entries().iter().map(|e| e.name.to_string()).collect();
            (pane.search.as_ref().map(|s| (s.running, s.stopped)), names)
        })
    };

    // A search that has already found something and is still running.
    let cancel = Arc::new(AtomicBool::new(false));
    let (worker, found, late) = (cancel.clone(), root.join("found.txt"), root.join("late.txt"));
    cx.update(|cx| {
        ws.update(cx, |ws, cx| {
            ws.pane().update(cx, |p, cx| {
                p.start_search("test".into(), cancel.clone(), cx, move |push, _| {
                    push(vec![rile_core::Entry::from_path(found).unwrap()]);
                    // Stops early once told to; otherwise it would find more.
                    if !worker.load(Ordering::Relaxed) {
                        push(vec![rile_core::Entry::from_path(late).unwrap()]);
                    }
                    Ok(())
                })
            })
        })
    });
    // Esc lands before the worker runs: the search stops, the worker sees
    // the flag, and what it found is kept.
    press(cx, window, &["escape"]);
    assert!(cancel.load(Ordering::Relaxed), "worker was told to stop");
    assert_eq!(state(cx), (Some((false, true)), vec!["found.txt".to_string()]));

    // Stopping a search that already shows results keeps them as they are.
    cx.update(|cx| ws.update(cx, |ws, cx| ws.pane().update(cx, |p, _| p.search.as_mut().unwrap().running = true)));
    press(cx, window, &["escape"]);
    assert_eq!(state(cx), (Some((false, true)), vec!["found.txt".to_string()]));

    // Once stopped, Esc clears the search and the folder comes back.
    press(cx, window, &["escape"]);
    wait_for_names(cx, window, &ws, &["found.txt", "late.txt"]).await;
    assert!(cx.update(|cx| ws.read(cx).pane().read(cx).search.is_none()));
}

/// Regression: Enter on a file read the workspace inside its own update and aborted.
#[gpui_kit::test]
async fn enter_on_file_does_not_crash(cx: &mut TestAppContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("a.txt"), "hello").unwrap();

    let (window, ws) = setup(&root, cx);
    // A bogus default app avoids the test platform's unimplemented
    // open_with_system while still reading the workspace's settings.
    let fake_app = tmp.path().join("Fake.app");
    fs::create_dir(&fake_app).unwrap();
    cx.update(|cx| ws.update(cx, |ws, _| ws.settings.default_apps.insert(".txt".into(), fake_app.clone())));
    wait_for_names(cx, window, &ws, &["Fake.app", "a.txt"]).await;
    press(cx, window, &["down", "down", "enter"]);
    wait_for_names(cx, window, &ws, &["Fake.app", "a.txt"]).await;
}
