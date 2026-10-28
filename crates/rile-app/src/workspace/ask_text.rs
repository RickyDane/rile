//! Renders an Ask answer character by character: each character fades in
//! and settles upwards as it is revealed, while words still wrap as a whole. Covers the Markdown small models write
//! (paragraphs, headings, lists, **bold**, *italic*, `code`, links); tables
//! and fenced code use the Markdown view and fade in as a whole.

use std::time::Duration;
use std::{path::PathBuf, rc::Rc};

use gpui_kit::component::{ActiveTheme, h_flex, text::TextView, v_flex};
use gpui_kit::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

const CHAR_FADE: Duration = Duration::from_millis(300);
const BLOCK_FADE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Default, PartialEq, Debug)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
}

/// A run of text in one style; a word is one or more runs without spaces.
#[derive(Clone, PartialEq, Debug)]
struct Run {
    text: String,
    style: Style,
    path: Option<PathBuf>,
}

pub type FileClick = Rc<dyn Fn(PathBuf, &ClickEvent, &mut Window, &mut App)>;

type Word = Vec<Run>;

#[derive(PartialEq, Debug)]
enum Line {
    Paragraph(Vec<Word>),
    Heading(Vec<Word>),
    Item { marker: String, indent: usize, words: Vec<Word> },
}

/// The answer as animated elements. `turn` keeps element ids unique per
/// question; ids count characters, so a character keeps its animation as
/// more text arrives after it.
pub fn render(turn: usize, text: &str, paths: &[PathBuf], on_file: FileClick, cx: &App) -> AnyElement {
    let ch = crate::theme::chrome(cx);
    let mut next = turn * 1_000_000;
    let mut id = move || {
        next += 1;
        next
    };
    let blocks = super::ask::markdown_blocks(text);
    v_flex()
        .gap(px(8.))
        .text_color(ch.text)
        .line_height(px(20.))
        .children(blocks.into_iter().map(|block| {
            let first = block.lines().next().unwrap_or_default().trim_start();
            if first.starts_with("```") || block.lines().all(|l| l.trim_start().starts_with('|')) {
                let block_id = id();
                return div()
                    .relative()
                    .child(TextView::markdown(("ask-answer-block", block_id), block).selectable(true))
                    .with_animation(("ask-block-fade", block_id), Animation::new(BLOCK_FADE).with_easing(gpui_kit::ease_in_out), |el, t| {
                        el.opacity(t).top(px(4. * (1. - t)))
                    })
                    .into_any_element();
            }
            v_flex()
                .gap(px(4.))
                .children(lines_with_paths(&block, paths).into_iter().map(|line| match line {
                    Line::Paragraph(words) => flow(words, &mut id, &on_file, cx).into_any_element(),
                    Line::Heading(words) => flow(words, &mut id, &on_file, cx).text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).into_any_element(),
                    Line::Item { marker, indent, words } => h_flex()
                        .items_start()
                        .pl(px(4. + 16. * indent as f32))
                        .gap(px(6.))
                        .child(fade(div().flex_none().min_w(px(10.)).text_color(ch.text_muted).child(marker), id()))
                        .child(flow(words, &mut id, &on_file, cx).flex_1().min_w_0())
                        .into_any_element(),
                }))
                .into_any_element()
        }))
        .into_any_element()
}

/// Words wrapping like text; each word is one unbreakable unit whose
/// characters fade in on their own.
fn flow(words: Vec<Word>, id: &mut impl FnMut() -> usize, on_file: &FileClick, cx: &App) -> gpui_kit::Div {
    let mono = cx.theme().mono_font_family.clone();
    let ch = crate::theme::chrome(cx);
    h_flex().flex_wrap().items_baseline().children(words.into_iter().map(|word| {
        let runs: Vec<_> = word
            .into_iter()
            .map(|run| {
                // Styles sit on the run, so a code span keeps one background.
                let chars: Vec<_> = run.text.chars().map(|c| fade(div().child(SharedString::from(c.to_string())), id())).collect();
                let mut el = h_flex()
                    .items_baseline()
                    .when_bold(run.style.bold)
                    .when_italic(run.style.italic)
                    .when_code(run.style.code, mono.clone(), ch.hover_strong)
                    .children(chars);
                if let Some(path) = run.path {
                    let on_file = on_file.clone();
                    let tip = format!("{} — Click to reveal; {}-click to open", path.display(), if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" });
                    return el.id(("ask-file", id())).cursor_pointer().text_color(ch.accent)
                        .hover(|s| s.underline())
                        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
                        .on_click(move |event, window, cx| on_file(path.clone(), event, window, cx))
                        .into_any_element();
                }
                el = el.text_color(ch.text);
                el.into_any_element()
            })
            .collect();
        // The trailing no-break space is the gap to the next word.
        h_flex().items_baseline().children(runs).child("\u{a0}")
    }))
}

fn fade(el: gpui_kit::Div, id: usize) -> impl IntoElement {
    el.relative()
        // Fades in while settling from the right and from below.
        .with_animation(("ask-char-fade", id), Animation::new(CHAR_FADE).with_easing(gpui_kit::ease_out_quint()), |el, t| {
            el.opacity(t).left(px(3. * (1. - t))).top(px(3. * (1. - t)))
        })
}

trait RunStyle {
    fn when_bold(self, on: bool) -> Self;
    fn when_italic(self, on: bool) -> Self;
    fn when_code(self, on: bool, mono: SharedString, bg: gpui_kit::Hsla) -> Self;
}

impl RunStyle for gpui_kit::Div {
    fn when_bold(self, on: bool) -> Self {
        if on { self.font_weight(FontWeight::SEMIBOLD) } else { self }
    }
    fn when_italic(self, on: bool) -> Self {
        if on { self.italic() } else { self }
    }
    fn when_code(self, on: bool, mono: SharedString, bg: gpui_kit::Hsla) -> Self {
        if on { self.font_family(mono).text_size(px(12.)).px(px(4.)).rounded(px(4.)).bg(bg) } else { self }
    }
}

/// A block's lines: headings, list items (continuation lines join the item)
/// and paragraphs (consecutive lines join).
#[cfg(test)]
fn lines(block: &str) -> Vec<Line> {
    lines_with_paths(block, &[])
}

fn lines_with_paths(block: &str, paths: &[PathBuf]) -> Vec<Line> {
    let words = |text: &str| words_with_paths(text, paths);
    let mut out: Vec<Line> = Vec::new();
    for raw in block.lines() {
        let indent = raw.chars().take_while(|c| *c == ' ').count() / 2;
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix('#') {
            out.push(Line::Heading(words(rest.trim_start_matches('#').trim())));
        } else if let Some((marker, rest)) = list_marker(line) {
            out.push(Line::Item { marker, indent, words: words(rest) });
        } else {
            match out.last_mut() {
                Some(Line::Paragraph(w)) | Some(Line::Item { words: w, .. }) => w.extend(words(line)),
                _ => out.push(Line::Paragraph(words(line))),
            }
        }
    }
    out
}

/// `- `, `* `, `+ ` (shown as a bullet) or `1. ` / `1) ` (kept).
fn list_marker(line: &str) -> Option<(String, &str)> {
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            return Some(("\u{2022}".into(), rest));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    let rest = &line[digits..];
    (digits > 0 && (rest.starts_with(". ") || rest.starts_with(") "))).then(|| (line[..digits + 1].to_string(), &rest[2..]))
}

/// Splits inline Markdown into words of styled runs. `**` and backticks
/// start their style even before the closing marker has streamed in, so
/// markers never flash up as text; a lone `*` or `_` without a partner stays
/// literal (it's rarely emphasis).
#[cfg(test)]
fn words(text: &str) -> Vec<Word> {
    words_with_paths(text, &[])
}

fn words_with_paths(text: &str, paths: &[PathBuf]) -> Vec<Word> {
    let mut mentions: Vec<_> = paths.iter().flat_map(|path| {
        [path.to_string_lossy().into_owned(), super::ai_agent::short(path)]
            .map(|s| (s.chars().collect::<Vec<_>>(), path))
    }).collect();
    mentions.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
    let chars: Vec<char> = text.chars().collect();
    let closes = |from: usize, marker: &[char]| (from..chars.len()).any(|j| chars[j..].starts_with(marker));
    let mut out: Vec<Word> = Vec::new();
    let mut word: Word = Vec::new();
    let mut run = String::new();
    let mut style = Style::default();
    let flush_run = |word: &mut Word, run: &mut String, style: Style| {
        if !run.is_empty() {
            word.push(Run { text: std::mem::take(run), style, path: None });
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some((mention, path)) = mentions.iter().find(|(s, _)| {
            (i == 0 || chars[i - 1].is_whitespace() || matches!(chars[i - 1], '*' | '(' | '[' | '"' | '\''))
                && chars[i..].starts_with(s)
                && {
                    let end = i + s.len();
                    let end = end + usize::from(chars.get(end) == Some(&'/'));
                    chars.get(end).is_none_or(|c| c.is_whitespace() || matches!(c, '`' | ',' | '.' | ':' | ';' | ')' | ']'))
                }
        }) {
            flush_run(&mut word, &mut run, style);
            word.push(Run { text: path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned(), style, path: Some((*path).clone()) });
            i += mention.len();
            if chars.get(i) == Some(&'/') { i += 1; }
        } else if c.is_whitespace() {
            flush_run(&mut word, &mut run, style);
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            i += 1;
        } else if c == '`' {
            // Code spans stay one unit, spaces included.
            flush_run(&mut word, &mut run, style);
            let end = (i + 1..chars.len()).find(|&j| chars[j] == '`').unwrap_or(chars.len());
            let inner: String = chars[i + 1..end].iter().collect();
            if let Some((_, path)) = mentions.iter().find(|(s, _)| s.iter().collect::<String>().trim_end_matches('/') == inner.trim_end_matches('/')) {
                word.push(Run { text: path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned(), style, path: Some((*path).clone()) });
            } else {
                word.push(Run { text: inner, style: Style { code: true, ..style }, path: None });
            }
            i = end + 1;
        } else if chars[i..].starts_with(&['*', '*']) {
            flush_run(&mut word, &mut run, style);
            style.bold = !style.bold;
            i += 2;
        } else if (c == '*' || (c == '_' && (i == 0 || !chars[i - 1].is_alphanumeric())) || (c == '_' && style.italic))
            && (style.italic || closes(i + 1, &[c]))
        {
            flush_run(&mut word, &mut run, style);
            style.italic = !style.italic;
            i += 1;
        } else if c == '[' {
            // File links display their basename; other links keep their label.
            let close = (i + 1..chars.len()).find(|&j| chars[j] == ']');
            match close.filter(|&j| chars.get(j + 1) == Some(&'(') && (j + 2..chars.len()).any(|k| chars[k] == ')')) {
                Some(j) => {
                    let url_end = if chars.get(j + 2) == Some(&'<') {
                        (j + 3..chars.len()).find(|&k| chars[k] == '>' && chars.get(k + 1) == Some(&')')).map(|k| k + 1)
                    } else {
                        let mut depth = 1;
                        (j + 2..chars.len()).find(|&k| {
                            if chars[k] == '(' { depth += 1; }
                            if chars[k] == ')' { depth -= 1; }
                            depth == 0
                        })
                    };
                    let Some(url_end) = url_end else { run.push(c); i += 1; continue; };
                    let inner: String = chars[i + 1..j].iter().collect();
                    let target: String = chars[j + 2..url_end].iter().collect();
                    let target = target.trim_matches(['<', '>']).trim_end_matches(['/', std::path::MAIN_SEPARATOR]);
                    if target.starts_with('/') || target.starts_with("~/") || target.starts_with("~\\") || std::path::Path::new(target).is_absolute() {
                        flush_run(&mut word, &mut run, style);
                        let path = super::expand_tilde(target);
                        word.push(Run { text: path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned(), style, path: Some(path) });
                        i = url_end + 1;
                        continue;
                    }
                    let inner = mentions.iter().find(|(s, _)| s.iter().collect::<String>().trim_end_matches('/') == target)
                        .map(|(_, path)| path.to_string_lossy().into_owned()).unwrap_or(inner);
                    let mut linked = words_with_paths(&inner, paths).into_iter();
                    if let Some(first) = linked.next() {
                        flush_run(&mut word, &mut run, style);
                        word.extend(first);
                        for w in linked {
                            out.push(std::mem::take(&mut word));
                            word = w;
                        }
                    }
                    i = url_end + 1;
                }
                None => {
                    run.push(c);
                    i += 1;
                }
            }
        } else {
            run.push(c);
            i += 1;
        }
    }
    flush_run(&mut word, &mut run, style);
    if !word.is_empty() {
        out.push(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(words: &[Word]) -> Vec<String> {
        words.iter().map(|w| w.iter().map(|r| r.text.as_str()).collect()).collect()
    }

    #[test]
    fn splits_inline_markdown_into_words() {
        let w = words("Read **the guide**, then `cargo run --release` and *relax* [docs](https://x.y).");
        assert_eq!(plain(&w), ["Read", "the", "guide,", "then", "cargo run --release", "and", "relax", "docs."]);
        assert!(w[1][0].style.bold && w[2][0].style.bold && !w[2][1].style.bold, "comma after bold isn't bold");
        assert!(w[4][0].style.code);
        assert!(w[6][0].style.italic);
        // Markers still waiting for their partner don't show up as text.
        let streaming = words("a **bol");
        assert_eq!(plain(&streaming), ["a", "bol"]);
        assert!(streaming[1][0].style.bold);
        assert_eq!(plain(&words("run `cargo")), ["run", "cargo"]);
        assert_eq!(plain(&words("2 * 3")), ["2", "*", "3"]);
        assert_eq!(plain(&words("snake_case_name")), ["snake_case_name"]);
    }

    #[test]
    fn reads_lists_headings_and_paragraphs() {
        let l = lines("## Files\n- one\n  more\n2. two\nplain");
        assert!(matches!(&l[0], Line::Heading(w) if plain(w) == ["Files"]));
        assert!(matches!(&l[1], Line::Item { marker, words, .. } if marker == "\u{2022}" && plain(words) == ["one", "more"]));
        assert!(matches!(&l[2], Line::Item { marker, .. } if marker == "2."));
        assert_eq!(l.len(), 3, "a plain line after an item continues it");
    }

    #[test]
    fn file_mentions_keep_full_targets_but_display_only_names() {
        let paths = vec![PathBuf::from("/tmp/Epic Games/Two-Factor codes.txt"), PathBuf::from("/tmp/Games")];
        for text in [
            "/tmp/Epic Games/Two-Factor codes.txt",
            "`/tmp/Epic Games/Two-Factor codes.txt`",
            "[full path](</tmp/Epic Games/Two-Factor codes.txt>)",
        ] {
            let w = words_with_paths(text, &paths);
            assert_eq!(plain(&w), ["Two-Factor codes.txt"]);
            assert_eq!(w[0][0].path.as_ref(), Some(&paths[0]));
        }
        let w = words_with_paths("/tmp/Games/", &paths);
        assert_eq!(plain(&w), ["Games"]);
        assert_eq!(w[0][0].path.as_ref(), Some(&paths[1]));
        assert!(words_with_paths("/tmp/Games/unknown.txt", &paths).iter().flatten().all(|r| r.path.is_none()));
    }

    #[test]
    fn markdown_file_links_support_spaces_parentheses_and_home_paths() {
        let w = words("[label](</tmp/My Game (2026).pdf>), [folder](~/Games)");
        assert_eq!(plain(&w), ["My Game (2026).pdf,", "Games"]);
        assert_eq!(w[0][0].path, Some(PathBuf::from("/tmp/My Game (2026).pdf")));
        assert_eq!(w[1][0].path, Some(super::super::expand_tilde("~/Games")));
        let w = words("[web](https://example.com)");
        assert_eq!(plain(&w), ["web"]);
        assert!(w[0][0].path.is_none());
    }
}
