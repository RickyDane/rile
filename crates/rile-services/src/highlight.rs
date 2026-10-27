//! Syntax highlighting for the Quick Look text preview, backed by syntect's
//! bundled syntaxes and themes (loaded once, lazily, on first use).
//!
//! Output is a list of styled byte ranges over the input text. Ranges drawn
//! in the theme's default foreground are *not* emitted: the UI paints those
//! with its own text color, so plain text stays consistent with the rest of
//! the interface and only the "interesting" tokens carry theme colors.

use std::ops::Range;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, FontStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// Only this many bytes of a text are highlighted; the rest is left plain.
pub const MAX_BYTES: usize = 512 * 1024;
/// Only this many lines of a text are highlighted; the rest is left plain.
pub const MAX_LINES: usize = 20_000;

const DARK_THEME: &str = "base16-ocean.dark";
const LIGHT_THEME: &str = "InspiredGitHub";

/// A styled byte range of the highlighted text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Byte range into the text passed to [`highlight`].
    pub range: Range<usize>,
    /// Foreground color as `0xRRGGBB`.
    pub color: u32,
    pub bold: bool,
    pub italic: bool,
}

struct Assets {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
}

fn assets() -> &'static Assets {
    static ASSETS: OnceLock<Assets> = OnceLock::new();
    ASSETS.get_or_init(|| Assets { syntaxes: SyntaxSet::load_defaults_newlines(), themes: ThemeSet::load_defaults() })
}

/// Resolves an extension (`"rs"`), a file name (`"Makefile"`, `"main.rs"`)
/// or a language token (`"rust"`) to a syntax. Plain text counts as none.
fn find_syntax<'a>(ss: &'a SyntaxSet, hint: &str) -> Option<&'a SyntaxReference> {
    let hint = hint.trim().trim_start_matches('.');
    if hint.is_empty() {
        return None;
    }
    let lower = hint.to_lowercase();
    let ext = hint.rsplit_once('.').map(|(_, e)| e).filter(|e| !e.is_empty());
    let syntax = ss
        .find_syntax_by_extension(hint)
        .or_else(|| ss.find_syntax_by_extension(&lower))
        .or_else(|| ext.and_then(|e| ss.find_syntax_by_extension(e).or_else(|| ss.find_syntax_by_extension(&e.to_lowercase()))))
        .or_else(|| ss.find_syntax_by_token(hint))?;
    (syntax.name != "Plain Text").then_some(syntax)
}

/// Human-readable language name for a hint (e.g. `"rs"` → `"Rust"`), for a
/// label in the preview. `None` if the hint matches no known syntax.
pub fn language_name(hint: &str) -> Option<String> {
    find_syntax(&assets().syntaxes, hint).map(|s| s.name.clone())
}

fn rgb(c: Color) -> u32 {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

/// The prefix of `text` that gets highlighted: at most [`MAX_BYTES`] bytes
/// (on a char boundary) and [`MAX_LINES`] lines.
fn highlight_limit(text: &str) -> &str {
    let mut end = text.len().min(MAX_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    match head.match_indices('\n').nth(MAX_LINES - 1) {
        Some((i, _)) => &head[..=i],
        None => head,
    }
}

/// Highlights `text` as the language named by `language_hint` (an extension
/// or file name such as `"rs"` or `"Makefile"`) with a dark or light theme.
///
/// Returns `None` when no syntax matches (or it's plain text). Spans are
/// sorted, non-overlapping and merged when adjacent with the same style.
/// Spans in the theme's default foreground with no bold/italic are omitted —
/// the UI draws those (and any gaps) in its own text color. Only the first
/// [`MAX_BYTES`] / [`MAX_LINES`] of the text are highlighted.
pub fn highlight(text: &str, language_hint: Option<&str>, dark: bool) -> Option<Vec<Span>> {
    let a = assets();
    let syntax = find_syntax(&a.syntaxes, language_hint?)?;
    let theme: &Theme = a.themes.themes.get(if dark { DARK_THEME } else { LIGHT_THEME })?;
    let default_fg = theme.settings.foreground.map(rgb);

    let mut h = HighlightLines::new(syntax, theme);
    let mut spans: Vec<Span> = Vec::new();
    let mut offset = 0;
    for line in LinesWithEndings::from(highlight_limit(text)) {
        let Ok(regions) = h.highlight_line(line, &a.syntaxes) else { break };
        for (style, piece) in regions {
            let start = offset;
            offset += piece.len();
            if piece.is_empty() {
                continue;
            }
            let color = rgb(style.foreground);
            let bold = style.font_style.contains(FontStyle::BOLD);
            let italic = style.font_style.contains(FontStyle::ITALIC);
            if Some(color) == default_fg && !bold && !italic {
                continue;
            }
            if let Some(last) = spans.last_mut()
                && last.range.end == start
                && last.color == color
                && last.bold == bold
                && last.italic == italic
            {
                last.range.end = offset;
                continue;
            }
            spans.push(Span { range: start..offset, color, bold, italic });
        }
    }
    Some(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_spans(text: &str, spans: &[Span]) {
        let mut prev = 0;
        for s in spans {
            assert!(s.range.start >= prev && s.range.start < s.range.end && s.range.end <= text.len());
            assert!(text.is_char_boundary(s.range.start) && text.is_char_boundary(s.range.end));
            prev = s.range.end;
        }
    }

    #[test]
    fn highlights_rust() {
        let text = "fn main() {\n    let x = \"héllo\"; // comment\n}\n";
        for dark in [true, false] {
            let spans = highlight(text, Some("rs"), dark).unwrap();
            assert!(!spans.is_empty());
            check_spans(text, &spans);
            assert!(spans.iter().any(|s| &text[s.range.clone()] == "fn"));
            assert!(spans.iter().any(|s| text[s.range.clone()].contains("comment")));
        }
        assert_eq!(language_name("rs").as_deref(), Some("Rust"));
        assert_eq!(language_name("main.rs").as_deref(), Some("Rust"));
    }

    #[test]
    fn highlights_json_and_python() {
        let json = "{\"key\": [1, true, null]}\n";
        let spans = highlight(json, Some("json"), true).unwrap();
        check_spans(json, &spans);
        assert!(spans.iter().any(|s| json[s.range.clone()].contains("key")));
        assert_eq!(language_name("JSON").as_deref(), Some("JSON"));

        let py = "def f(x):\n    return x + 1  # add\n";
        let spans = highlight(py, Some("py"), false).unwrap();
        check_spans(py, &spans);
        assert!(spans.iter().any(|s| &py[s.range.clone()] == "def"));
        assert_eq!(language_name("py").as_deref(), Some("Python"));
        assert_eq!(language_name("Makefile").as_deref(), Some("Makefile"));
    }

    #[test]
    fn unknown_and_plain_are_none() {
        assert!(highlight("hello", Some("zzzunknown"), true).is_none());
        assert!(highlight("hello", Some("txt"), true).is_none());
        assert!(highlight("hello", None, true).is_none());
        assert!(highlight("hello", Some(""), true).is_none());
        assert!(language_name("zzzunknown").is_none());
    }

    #[test]
    fn caps_large_inputs() {
        let text = "let a = 1;\n".repeat(MAX_LINES + 100);
        let spans = highlight(&text, Some("rs"), true).unwrap();
        let limit = highlight_limit(&text).len();
        assert!(limit < text.len());
        assert!(spans.iter().all(|s| s.range.end <= limit));
    }
}
