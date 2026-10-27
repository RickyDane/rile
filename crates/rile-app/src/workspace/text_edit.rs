//! Editing plain text, Markdown and RTF files right in the Quick Look card.
//!
//! RTF is edited as its plain text (macOS `textutil` converts both ways), so
//! saving keeps the words but drops rich formatting.

use std::path::{Path, PathBuf};

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, WindowExt, v_flex};
use gpui_kit::{prelude::FluentBuilder, AnyElement, AppContext, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Styled, Window, div, px};

use super::Workspace;

/// Largest file opened for editing.
const MAX_EDIT: u64 = 2 * 1024 * 1024;

const PLAIN: &[&str] = &[
    "txt", "text", "md", "markdown", "mdown", "mkd", "log", "csv", "tsv", "ini", "cfg", "conf", "yaml", "yml", "toml", "json", "xml",
];
const MARKDOWN: &[&str] = &["md", "markdown", "mdown", "mkd"];

fn ext(path: &Path) -> Option<String> {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase())
}

pub fn is_markdown(path: &Path) -> bool {
    ext(path).is_some_and(|e| MARKDOWN.contains(&e.as_str()))
}

pub fn is_rtf(path: &Path) -> bool {
    ext(path).is_some_and(|e| e == "rtf")
}

/// Whether the file can be edited in Quick Look.
pub fn is_editable(path: &Path) -> bool {
    let Some(e) = ext(path) else { return false };
    let known = PLAIN.contains(&e.as_str()) || (e == "rtf" && cfg!(target_os = "macos"));
    known && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_EDIT && !m.permissions().readonly())
}

/// The file's editable text (RTF: its plain text).
pub fn read_text(path: &Path) -> anyhow::Result<String> {
    if is_rtf(path) {
        let out = std::process::Command::new("textutil").args(["-convert", "txt", "-stdout"]).arg(path).output()?;
        anyhow::ensure!(out.status.success(), "textutil: {}", String::from_utf8_lossy(&out.stderr).trim());
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    Ok(String::from_utf8(std::fs::read(path)?)?)
}

pub fn write_text(path: &Path, text: &str) -> anyhow::Result<()> {
    if is_rtf(path) {
        use std::io::Write;
        let mut child = std::process::Command::new("textutil")
            .args(["-stdin", "-format", "txt", "-convert", "rtf", "-output"])
            .arg(path)
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        child.stdin.take().expect("piped stdin").write_all(text.as_bytes())?;
        let out = child.wait_with_output()?;
        anyhow::ensure!(out.status.success(), "textutil: {}", String::from_utf8_lossy(&out.stderr).trim());
        return Ok(());
    }
    std::fs::write(path, text)?;
    Ok(())
}

pub struct TextEditor {
    pub path: PathBuf,
    pub state: Entity<TextareaState>,
    original: String,
    /// Last save failure, shown under the editor.
    error: Option<String>,
}

impl TextEditor {
    pub fn dirty(&self, cx: &gpui_kit::App) -> bool {
        self.state.read(cx).value().as_ref() != self.original
    }
}

impl Workspace {
    pub(crate) fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.preview.target.clone().filter(|_| self.preview.target_conn.is_none()) else { return };
        let text = match read_text(&path) {
            Ok(t) => t,
            Err(e) => {
                window.push_notification(Notification::error(format!("Can't edit: {e:#}")), cx);
                return;
            }
        };
        let state = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx);
            s.set_value(text.clone(), window, cx);
            s
        });
        state.update(cx, |s, cx| s.focus(window, cx));
        self.preview.editor = Some(TextEditor { path, state, original: text, error: None });
        cx.notify();
    }

    /// Saves pending edits (if any) and leaves edit mode. Returns false when
    /// saving failed (the editor then stays open).
    pub(crate) fn finish_editing(&mut self, save: bool, cx: &mut Context<Self>) -> bool {
        let Some(ed) = self.preview.editor.as_mut() else { return true };
        if save && ed.dirty(cx) {
            let text = ed.state.read(cx).value().to_string();
            if let Err(e) = write_text(&ed.path, &text) {
                ed.error = Some(format!("Couldn't save: {e:#}"));
                cx.notify();
                return false;
            }
            // Reload the preview from disk.
            if self.preview.target.as_ref() == Some(&ed.path) {
                self.preview.loaded = None;
                self.preview.target = None;
            }
        }
        self.preview.editor = None;
        self.sync_preview(cx);
        cx.notify();
        true
    }

    pub(crate) fn render_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.preview.editor.as_ref()?;
        let theme = cx.theme().clone();
        let rtf = is_rtf(&ed.path);
        Some(
            v_flex()
                .id("ql-editor")
                .size_full()
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    let k = &ev.keystroke;
                    if k.key == "s" && k.modifiers.secondary(cx) && !k.modifiers.shift {
                        cx.stop_propagation();
                        this.save_in_place(cx);
                    } else if k.key == "escape" {
                        cx.stop_propagation();
                        this.leave_editor(true, window, cx);
                    }
                }))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .font_family(if is_markdown(&ed.path) || !rtf { theme.mono_font_family.clone() } else { theme.font_family.clone() })
                        .text_sm()
                        .child(Textarea::new(&ed.state).appearance(false).h_full()),
                )
                .when_some(ed.error.clone(), |d, e| d.child(div().px_3().py(px(4.)).text_xs().text_color(theme.danger).child(e)))
                .when(rtf, |d| {
                    d.child(
                        div()
                            .px_3()
                            .py(px(4.))
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("RTF is edited as plain text: saving keeps the words but removes formatting."),
                    )
                })
                .into_any_element(),
        )
    }

    /// Leaves edit mode from the UI and hands keyboard focus back to the
    /// file list, so Space/Esc/arrows work on Quick Look again.
    pub(crate) fn leave_editor(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.finish_editing(save, cx) {
            let focus = self.pane().read(cx).focus_handle.clone();
            focus.focus(window, cx);
        }
    }

    /// ⌘S: writes the file and keeps editing.
    fn save_in_place(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.preview.editor.as_mut() else { return };
        if !ed.dirty(cx) {
            return;
        }
        let text = ed.state.read(cx).value().to_string();
        match write_text(&ed.path, &text) {
            Ok(()) => {
                ed.original = text;
                ed.error = None;
            }
            Err(e) => ed.error = Some(format!("Couldn't save: {e:#}")),
        }
        cx.notify();
    }
}
