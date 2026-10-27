//! Editing code, plain text, Markdown and RTF in the Quick Look card.
//!
//! RTF is edited as its plain text (macOS `textutil` converts both ways), so
//! saving keeps the words but drops rich formatting.

use std::path::{Path, PathBuf};

use gpui_kit::component::input::{
    AnyInputState, Editor, EditorState, FoldRange, HighlightStyleResolver, InputEdit,
    InputHighlighter, Rope, Textarea, TextareaState,
};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme, WindowExt, v_flex};
use gpui_kit::{prelude::FluentBuilder, AnyElement, AppContext, Context, HighlightStyle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, SharedString, Styled, Window, div, px};

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

/// The Kit editor accepts syntax names or extensions. Keep prose in Textarea.
fn code_language(path: &Path) -> Option<String> {
    let hint = ext(path).or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))?;
    match hint.as_str() {
        "txt" | "text" | "log" | "csv" | "tsv" | "rtf" => None,
        e if MARKDOWN.contains(&e) => None,
        "slnx" | "csproj" | "fsproj" | "vbproj" | "vcxproj" | "props" | "targets" | "resx" | "config" => Some("xml".into()),
        "slnf" => Some("json".into()),
        "ts" | "tsx" | "jsx" | "jsonc" | "fs" | "fsx" | "vb" => Some(hint),
        _ => rile_services::highlight::language_name(&hint).map(|_| hint),
    }
}

/// Whether the file can be edited in Quick Look.
pub fn is_editable(path: &Path) -> bool {
    let known = code_language(path).is_some() || ext(path).is_some_and(|e| {
        PLAIN.contains(&e.as_str()) || (e == "rtf" && cfg!(target_os = "macos"))
    });
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
    pub state: AnyInputState,
    original: String,
    /// Last save failure, shown under the editor.
    error: Option<String>,
}

impl TextEditor {
    pub fn dirty(&self, cx: &gpui_kit::App) -> bool {
        self.state.value(cx).as_ref() != self.original
    }
}

/// Supply .sln semantic runs through Kit's highlighter seam so colors follow
/// the active Kit theme.
#[derive(Default)]
struct SolutionHighlighter {
    tokens: Vec<(std::ops::Range<usize>, &'static str)>,
}

impl InputHighlighter for SolutionHighlighter {
    fn language(&self) -> SharedString { "sln".into() }

    fn update(&mut self, _: Option<InputEdit>, text: &Rope, _: bool, _: &mut Window, _: &mut Context<EditorState>) {
        self.tokens = rile_services::highlight::solution_tokens(&text.to_string());
    }

    fn styles(&self, range: &std::ops::Range<usize>, resolver: &dyn HighlightStyleResolver) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
        let mut runs = Vec::new();
        let mut offset = range.start;
        let first = self.tokens.partition_point(|(token, _)| token.end <= range.start);
        for (token, kind) in &self.tokens[first..] {
            if token.start >= range.end { break; }
            let start = token.start.max(range.start);
            let end = token.end.min(range.end);
            if start >= end { continue; }
            if offset < start { runs.push((offset..start, HighlightStyle::default())); }
            runs.push((start..end, resolver.style(kind).unwrap_or_default()));
            offset = end;
        }
        if offset < range.end { runs.push((offset..range.end, HighlightStyle::default())); }
        runs
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> { Vec::new() }
}

impl Workspace {
    pub(crate) fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.preview.target.clone().filter(|p| self.preview.target_conn.is_none() && is_editable(p)) else { return };
        let text = match read_text(&path) {
            Ok(t) => t,
            Err(e) => {
                window.push_notification(Notification::error(format!("Can't edit: {e:#}")), cx);
                return;
            }
        };
        let state: AnyInputState = if let Some(language) = code_language(&path) {
            cx.new(|cx| {
                let mut s = EditorState::new(window, cx).language(language.clone()).line_number(true);
                if language == "sln" {
                    s.set_highlighter_factory(std::rc::Rc::new(|_| Some(Box::new(SolutionHighlighter::default()))), cx);
                }
                s.set_value(text.clone(), window, cx);
                s
            }).into()
        } else {
            cx.new(|cx| {
                let mut s = TextareaState::new(window, cx);
                s.set_value(text.clone(), window, cx);
                s
            }).into()
        };
        state.focus_handle(cx).focus(window, cx);
        self.preview.editor = Some(TextEditor { path, state, original: text, error: None });
        cx.notify();
    }

    /// Saves pending edits (if any) and leaves edit mode. Returns false when
    /// saving failed (the editor then stays open).
    pub(crate) fn finish_editing(&mut self, save: bool, cx: &mut Context<Self>) -> bool {
        let Some(ed) = self.preview.editor.as_mut() else { return true };
        if save && ed.dirty(cx) {
            let text = ed.state.value(cx).to_string();
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
                        .child(match &ed.state {
                            AnyInputState::Editor(state) => Editor::new(state).appearance(false).h_full().into_any_element(),
                            AnyInputState::Textarea(state) => Textarea::new(state).appearance(false).h_full().into_any_element(),
                            _ => unreachable!("Quick Look only creates editors and textareas"),
                        }),
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
        let text = ed.state.value(cx).to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_code_editor_for_sources_and_dotnet_files() {
        for (name, language) in [
            ("main.rs", "rs"), ("App.CS", "cs"), ("App.SLN", "sln"),
            ("App.slnx", "xml"), ("App.slnf", "json"), ("App.csproj", "xml"), ("App.fsproj", "xml"),
            ("App.vbproj", "xml"), ("Directory.Build.props", "xml"),
            ("Directory.Build.targets", "xml"), ("NuGet.Config", "xml"),
            ("main.ts", "ts"), ("Makefile", "Makefile"),
        ] {
            assert_eq!(code_language(Path::new(name)).as_deref(), Some(language), "{name}");
        }
        for name in ["notes.md", "notes.MARKDOWN", "notes.txt", "notes.rtf", "photo.png", "unknown.xyzunknown"] {
            assert!(code_language(Path::new(name)).is_none(), "{name}");
        }
    }

    #[test]
    fn code_editability_keeps_file_and_size_guards() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("App.sln");
        assert!(!is_editable(&path));
        std::fs::write(&path, "Global\nEndGlobal\n").unwrap();
        assert!(is_editable(&path));
        std::fs::File::options().write(true).open(&path).unwrap().set_len(MAX_EDIT + 1).unwrap();
        assert!(!is_editable(&path));
        let directory = tmp.path().join("directory.rs");
        std::fs::create_dir(&directory).unwrap();
        assert!(!is_editable(&directory));
    }

    #[test]
    fn solution_styles_cover_requested_range_and_follow_resolver() {
        struct Resolver;
        impl HighlightStyleResolver for Resolver {
            fn style(&self, _: &str) -> Option<HighlightStyle> {
                Some(HighlightStyle { color: Some(gpui_kit::rgb(0x123456).into()), ..Default::default() })
            }
        }
        let text = "Project(\"héllo\")\nEndProject\n";
        let highlighter = SolutionHighlighter { tokens: rile_services::highlight::solution_tokens(text) };
        for range in [0..text.len(), 2..10, text.len()..text.len() + 10] {
            let runs = highlighter.styles(&range, &Resolver);
            let mut offset = range.start;
            for (run, _) in &runs {
                assert_eq!(run.start, offset);
                assert!(run.end > run.start && run.end <= range.end);
                offset = run.end;
            }
            assert_eq!(offset, range.end);
        }
        assert!(highlighter.styles(&(0..text.len()), &Resolver).iter().any(|(_, style)| style.color.is_some()));
        assert!(highlighter.styles(&(0..0), &Resolver).is_empty());
    }
}
