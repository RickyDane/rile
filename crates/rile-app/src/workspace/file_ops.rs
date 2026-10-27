//! File operations: clipboard, transfers, trash/delete, rename, new folder.

use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui_kit::component::{WindowExt, input::{InputEvent, InputState}, notification::Notification};
use gpui_kit::{App, AppContext, ClipboardEntry, ClipboardItem, Context, SharedString, Window};
use rile_core::ops::{self, Progress, TransferKind};
use rile_core::ClipboardMode;

use super::{RunningOp, Workspace, display_name, item_count};
use crate::actions::*;
use crate::dialogs;

impl Workspace {

    /// Runs `work` on a background thread with a status-bar progress entry.
    /// Paths it returns get selected once the active pane reloads.
    pub fn start_op(
        &mut self,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce(&AtomicBool, &mut dyn FnMut(Progress)) -> Result<Vec<PathBuf>> + Send + 'static,
    ) {
        self.start_op_then(label, window, cx, work, |_, _, _| {});
    }

    /// Like [`Self::start_op`], running `then` on the UI thread after success.
    pub fn start_op_then(
        &mut self,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        work: impl FnOnce(&AtomicBool, &mut dyn FnMut(Progress)) -> Result<Vec<PathBuf>> + Send + 'static,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let id = self.next_op_id;
        self.next_op_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = smol::channel::unbounded::<Progress>();
        let worker_cancel = cancel.clone();
        let job = cx.background_spawn(async move {
            let mut last = Instant::now();
            let mut report = |p: Progress| {
                // Throttle UI updates to ~30/s.
                if last.elapsed() >= Duration::from_millis(33) || p.bytes_done == p.bytes_total {
                    last = Instant::now();
                    tx.try_send(p).ok();
                }
            };
            work(&worker_cancel, &mut report)
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(p) = rx.recv().await {
                let updated = this.update(cx, |this, cx| {
                    if let Some(op) = this.ops.iter_mut().find(|o| o.id == id) {
                        op.update_progress(p);
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                if let Some(ix) = this.ops.iter().position(|o| o.id == id) {
                    let op = this.ops.remove(ix);
                    let error = match &result {
                        Ok(_) => None,
                        Err(e) => Some(SharedString::from(format!("{e:#}"))),
                    };
                    this.finished_jobs.insert(0, super::FinishedJob { label: op.label, error, finished: Instant::now() });
                    this.finished_jobs.truncate(6);
                }
                match result {
                    Ok(created) => {
                        this.pane().update(cx, |p, cx| {
                            if !created.is_empty() {
                                p.select_after_reload(created);
                            }
                            p.reload(cx);
                        });
                        then(this, window, cx);
                    }
                    Err(e) if e.to_string() == "Cancelled" => {}
                    Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
                }
                cx.notify();
            })
            .ok();
        });
        self.ops.push(RunningOp {
            id,
            label: label.into(),
            progress: Progress::default(),
            cancel,
            started: Instant::now(),
            backgrounded: false,
            speed: 0.,
            last_sample: None,
            _task: task,
        });
        // Long transfers open the transfer window after a second, even if
        // no progress arrives in the meantime.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1050)).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
        cx.notify();
    }

    pub fn cancel_op(&mut self, id: usize) {
        if let Some(op) = self.ops.iter().find(|o| o.id == id) {
            op.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn set_clipboard(&mut self, mode: ClipboardMode, cx: &mut Context<Self>) {
        let paths = self.selection(cx);
        if paths.is_empty() {
            return;
        }
        // Also expose paths as text so they paste into terminals/editors.
        let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.clipboard.set(paths, mode);
        self.clipboard_conn = self.pane().read(cx).conn();
        self.sync_cut_paths(cx);
    }

    pub(crate) fn sync_cut_paths(&self, cx: &mut Context<Self>) {
        let cut: std::collections::HashSet<PathBuf> = match &self.clipboard.items {
            Some((paths, ClipboardMode::Cut)) => paths.iter().cloned().collect(),
            _ => Default::default(),
        };
        for pane in &self.all_panes() {
            pane.update(cx, |p, cx| {
                p.cut_paths = cut.clone();
                cx.notify();
            });
        }
    }

    pub(crate) fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.set_clipboard(ClipboardMode::Copy, cx);
    }

    pub(crate) fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        self.set_clipboard(ClipboardMode::Cut, cx);
    }

    pub(crate) fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let (dest, dest_conn) = {
            let pane = self.pane().read(cx);
            (pane.dir_path(), pane.conn())
        };
        let Some(dest) = dest else { return };
        // Prefer a newly copied image over stale app-internal file paths.
        let image = cx.read_from_clipboard().and_then(|item| {
            item.into_entries().find_map(|entry| match entry {
                ClipboardEntry::Image(image) => Some(image),
                _ => None,
            })
        });
        if let Some(image) = image {
            if dest_conn.is_some() {
                window.push_notification(Notification::info("Pasting images isn't available on servers"), cx);
                return;
            }
            self.clipboard.items = None;
            self.clipboard_conn = None;
            self.sync_cut_paths(cx);
            self.start_op("Pasting image", window, cx, move |cancel, _| {
                anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
                let decoded = image::load_from_memory(image.bytes()).context("decoding clipboard image")?;
                let mut pixels = decoded.to_rgba8();
                // JPEG has no alpha channel; composite transparency onto white.
                for pixel in pixels.pixels_mut() {
                    let alpha = u32::from(pixel[3]);
                    for channel in &mut pixel.0[..3] {
                        *channel = ((u32::from(*channel) * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
                    }
                    pixel[3] = 255;
                }
                let mut jpeg = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
                    .encode_image(&image::DynamicImage::ImageRgba8(pixels).to_rgb8())
                    .context("encoding clipboard image as JPEG")?;
                let path = ops::unique_destination(&dest, "pasted image.jpeg");
                let mut file = std::fs::File::create_new(&path)
                    .with_context(|| format!("creating {}", path.display()))?;
                if let Err(error) = file.write_all(&jpeg).and_then(|_| file.flush()) {
                    drop(file);
                    std::fs::remove_file(&path).ok();
                    return Err(error).with_context(|| format!("writing {}", path.display()));
                }
                Ok(vec![path])
            });
            return;
        }
        let src_conn = self.clipboard_conn.clone();
        let Some((sources, mode)) = self.clipboard.take_for_paste().or_else(|| system_clipboard_paths(cx)) else { return };
        self.sync_cut_paths(cx);
        if src_conn.is_some() || dest_conn.is_some() {
            self.transfer_between(sources, src_conn, dest, dest_conn, mode == ClipboardMode::Copy, window, cx);
            return;
        }
        self.transfer(sources, dest, mode == ClipboardMode::Copy, window, cx);
    }

    pub(crate) fn duplicate(&mut self, _: &Duplicate, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane().read(cx).conn().is_some() {
            window.push_notification(Notification::info("Duplicate isn't available on servers"), cx);
            return;
        }
        let paths = self.selection(cx);
        if paths.is_empty() {
            return;
        }
        let label = format!("Duplicating {}", item_count(paths.len()));
        self.start_op(label, window, cx, move |cancel, _| ops::duplicate(&paths, cancel));
    }

    pub(crate) fn move_to_trash(&mut self, _: &MoveToTrash, window: &mut Window, cx: &mut Context<Self>) {
        // Servers have no trash: deleting there always asks first.
        if self.pane().read(cx).conn().is_some() {
            self.delete_permanently(&DeletePermanently, window, cx);
            return;
        }
        let paths = self.selection(cx);
        if paths.is_empty() {
            return;
        }
        if self.settings.confirm_trash {
            let ws = cx.entity().downgrade();
            dialogs::confirm(
                format!("Move {} to the Trash?", item_count(paths.len())),
                "Move to Trash",
                false,
                window,
                cx,
                move |window, cx| {
                    let paths = paths.clone();
                    ws.update(cx, |this, cx| this.trash_paths(paths, window, cx)).ok();
                },
            );
        } else {
            self.trash_paths(paths, window, cx);
        }
    }

    pub(crate) fn trash_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Moving {} to Trash", item_count(paths.len()));
        self.start_op(label, window, cx, move |_, _| ops::move_to_trash(&paths).map(|_| Vec::new()));
    }

    pub(crate) fn delete_permanently(&mut self, _: &DeletePermanently, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selection(cx);
        if paths.is_empty() {
            return;
        }
        if let Some(conn) = self.pane().read(cx).conn() {
            let ws = cx.entity().downgrade();
            let msg = format!("Delete {} from {}? This can't be undone.", item_count(paths.len()), crate::remote::label(&conn));
            dialogs::confirm(msg, "Delete", true, window, cx, move |window, cx| {
                let (conn, paths) = (conn.clone(), paths.clone());
                ws.update(cx, |ws, cx| ws.remote_delete(conn, paths, window, cx)).ok();
            });
            return;
        }
        let ws = cx.entity().downgrade();
        dialogs::confirm(
            format!("Permanently delete {}? This can't be undone.", item_count(paths.len())),
            "Delete",
            true,
            window,
            cx,
            move |window, cx| {
                let paths = paths.clone();
                ws.update(cx, |this, cx| {
                    let label = format!("Deleting {}", item_count(paths.len()));
                    this.start_op(label, window, cx, move |cancel, report| {
                        let mut p = Progress { items_total: paths.len(), ..Default::default() };
                        for path in &paths {
                            if cancel.load(Ordering::Relaxed) {
                                anyhow::bail!("Cancelled");
                            }
                            ops::remove_permanently(path)?;
                            p.items_done += 1;
                            report(p.clone());
                        }
                        Ok(Vec::new())
                    });
                })
                .ok();
            },
        );
    }

    pub(crate) fn rename(&mut self, _: &Rename, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selection(cx).first().cloned() {
            self.rename_path(path, window, cx);
        }
    }

    pub fn rename_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let conn = self.pane().read(cx).conn();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let ws = cx.entity().downgrade();
        let pane = self.pane().downgrade();
        // `submit` runs both from the dialog's OK button and from the input's
        // Enter subscription, which fires *inside* a workspace update — so it
        // must never update the workspace synchronously.
        let submit = {
            let input = input.clone();
            let path = path.clone();
            move |window: &mut Window, cx: &mut App| {
                let new_name = input.read(cx).value().trim().to_string();
                if let Some(conn) = conn.clone() {
                    let (ws, path) = (ws.clone(), path.clone());
                    window.defer(cx, move |window, cx| {
                        ws.update(cx, |ws, cx| ws.remote_rename(conn, path, new_name, window, cx)).ok();
                    });
                    return true;
                }
                match ops::rename(&path, &new_name) {
                    Ok(dest) => {
                        pane.update(cx, |p, cx| {
                            p.select_after_reload(vec![dest]);
                            p.reload(cx);
                        })
                        .ok();
                        true
                    }
                    Err(e) => {
                        window.push_notification(Notification::error(format!("{e:#}")), cx);
                        false
                    }
                }
            }
        };
        self._dialog_sub = Some(cx.subscribe_in(&input, window, {
            let submit = submit.clone();
            move |_, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event
                    && submit(window, cx)
                {
                    window.close_dialog(cx);
                }
            }
        }));
        let first = Cell::new(true);
        let focus_pane = self.pane().read(cx).focus_handle.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |i, cx| {
                    i.focus(window, cx);
                    i.select_all(window, cx);
                });
            }
            let submit = submit.clone();
            let focus_pane = focus_pane.clone();
            dialogs::input_dialog(dialog, "Rename", &input, move |window, cx| submit(window, cx), cx)
                .on_close(move |_, window, cx| focus_pane.focus(window, cx))
        });
    }

    pub(crate) fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane().read(cx).conn().is_some() {
            self.remote_create(false, window, cx);
            return;
        }
        let Some(dir) = self.current_dir(cx) else { return };
        match ops::create_file(&dir, "untitled.txt") {
            Ok(path) => {
                self.pane().update(cx, |p, cx| {
                    p.select_after_reload(vec![path.clone()]);
                    p.reload(cx);
                });
                self.rename_path(path, window, cx);
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    pub(crate) fn new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane().read(cx).conn().is_some() {
            self.remote_create(true, window, cx);
            return;
        }
        let Some(dir) = self.current_dir(cx) else { return };
        match ops::create_folder(&dir, "untitled folder") {
            Ok(path) => {
                self.pane().update(cx, |p, cx| {
                    p.select_after_reload(vec![path.clone()]);
                    p.reload(cx);
                });
                self.rename_path(path, window, cx);
            }
            Err(e) => window.push_notification(Notification::error(format!("{e:#}")), cx),
        }
    }

    pub(crate) fn toggle_favorite(&mut self, _: &ToggleFavorite, window: &mut Window, cx: &mut Context<Self>) {
        let dirs: Vec<PathBuf> = self
            .pane()
            .read(cx)
            .selected_entries()
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| e.path.clone())
            .collect();
        let targets = if dirs.is_empty() { self.current_dir(cx).into_iter().collect() } else { dirs };
        let added = targets.iter().any(|p| !self.settings.favorites.contains(p));
        for p in targets {
            self.settings.toggle_favorite(p);
        }
        if added {
            self.reward_event(rile_services::rewards::Event::Personalized, cx);
        }
        self.save_settings(window, cx);
        cx.notify();
    }


    /// Copies or moves `paths` into `dest` as a background operation.
    pub fn transfer(&mut self, paths: Vec<PathBuf>, dest: PathBuf, copy: bool, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let kind = if copy { TransferKind::Copy } else { TransferKind::Move };
        let dual_transfer = self.dual && self.other_dir(cx).is_some_and(|(other, conn)| conn.is_none() && other == dest)
            && paths.iter().any(|p| p.parent() != Some(dest.as_path()));
        let label = format!(
            "{} {} to {}",
            if copy { "Copying" } else { "Moving" },
            item_count(paths.len()),
            display_name(&dest)
        );
        self.start_op(label, window, cx, move |cancel, report| {
            // Only count actual selected files moved to a different folder,
            // not no-op moves, copies, or guessed recursive totals.
            let organized = if copy { 0 } else { paths.iter().filter(|p| p.is_file() && p.parent() != Some(dest.as_path())).count() };
            let created = ops::transfer(&paths, &dest, kind, cancel, report)?;
            if !cancel.load(Ordering::Relaxed) {
                if organized > 0 { rile_services::rewards::record_event(rile_services::rewards::Event::Organized(organized)); }
                if dual_transfer { rile_services::rewards::record_event(rile_services::rewards::Event::DualTransfer); }
            }
            Ok(created)
        });
    }

    /// The other pane's directory and connection (dual-pane mode).
    pub(crate) fn other_dir(&self, cx: &App) -> Option<(PathBuf, Option<String>)> {
        self.other_pane().and_then(|p| {
            let p = p.read(cx);
            p.dir_path().map(|d| (d, p.conn()))
        })
    }

    pub fn copy_to_other(&mut self, _: &CopyToOtherPane, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_transfer_to_other(true, window, cx);
    }

    pub fn move_to_other(&mut self, _: &MoveToOtherPane, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_transfer_to_other(false, window, cx);
    }

    fn confirm_transfer_to_other(&mut self, copy: bool, window: &mut Window, cx: &mut Context<Self>) {
        let verb = if copy { "Copy" } else { "Move" };
        let Some((dest, dest_conn)) = self.other_dir(cx) else {
            window.push_notification(Notification::info(format!("Open dual-pane view (⇧⌘D) to {} to the other pane", verb.to_lowercase())), cx);
            return;
        };
        let paths = self.selection(cx);
        let src_conn = self.pane().read(cx).conn();
        self.confirm_transfer(paths, src_conn, dest, dest_conn, copy, window, cx);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn confirm_transfer(
        &mut self, paths: Vec<PathBuf>, src_conn: Option<String>, dest: PathBuf,
        dest_conn: Option<String>, copy: bool, window: &mut Window, cx: &mut Context<Self>,
    ) {
        if paths.is_empty() { return; }
        let verb = if copy { "Copy" } else { "Move" };
        let destination = match &dest_conn {
            Some(conn) => format!("{}:{}", crate::remote::label(conn), dest.display()),
            None => dest.display().to_string(),
        };
        let ws = cx.entity().downgrade();
        dialogs::confirm(
            format!("{verb} {} to:\n{destination}?", item_count(paths.len())),
            verb, false, window, cx,
            move |window, cx| {
                _ = ws.update(cx, |this, cx| {
                    this.transfer_between(paths.clone(), src_conn.clone(), dest.clone(), dest_conn.clone(), copy, window, cx);
                });
            },
        );
    }

    /// Copies the selected paths, or the current folder's when nothing is selected.
    pub fn copy_path(&mut self, _: &CopyPath, _: &mut Window, cx: &mut Context<Self>) {
        let mut paths = self.selection(cx);
        if paths.is_empty() {
            paths.extend(self.pane().read(cx).dir_path());
        }
        let text = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Unmounts/ejects a volume, leaving it first if a pane is inside.
    pub fn unmount(&mut self, mount: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        for pane in self.all_panes() {
            let inside = pane.read(cx).current_dir().is_some_and(|d| d.starts_with(&mount));
            if inside {
                pane.update(cx, |p, cx| p.navigate(rile_core::history::Location::Home, cx));
            }
        }
        let name = display_name(&mount);
        let label = format!("Ejecting {name}");
        self.start_op_then(
            label,
            window,
            cx,
            move |_, _| {
                rile_services::volumes::unmount(&mount, true)?;
                Ok(Vec::new())
            },
            move |this, window, cx| {
                this.refresh_volumes(cx);
                window.push_notification(Notification::success(format!("\"{name}\" can be safely removed")), cx);
            },
        );
    }
}

/// Paths copied as text elsewhere (one per line), if they all exist.
fn system_clipboard_paths(cx: &App) -> Option<(Vec<PathBuf>, ClipboardMode)> {
    let text = cx.read_from_clipboard()?.text()?;
    let paths: Vec<PathBuf> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect();
    (!paths.is_empty() && paths.iter().all(|p| p.is_absolute() && p.exists())).then_some((paths, ClipboardMode::Copy))
}
