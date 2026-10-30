//! Full-color symbols for sidebar places and folders in the file list
//! (approved in mockups/rich-symbols.html). Depending on
//! `Settings::icon_color` they are drawn in color, recolored in the accent
//! color, or replaced by the icon style's outline glyphs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::{AnyElement, Hsla, Image, IntoElement, ObjectFit, Styled, StyledImage, img, px};
use rile_core::settings::IconColor;
use rile_core::{Entry, EntryKind};

/// The symbol mode in use (index into [`IconColor::ALL`]).
static ICON_COLOR: AtomicU8 = AtomicU8::new(1);

pub fn set_icon_color(mode: IconColor) {
    let ix = IconColor::ALL.iter().position(|m| *m == mode).unwrap_or(0);
    ICON_COLOR.store(ix as u8, Ordering::Relaxed);
}

pub fn icon_color() -> IconColor {
    IconColor::ALL[ICON_COLOR.load(Ordering::Relaxed) as usize % IconColor::ALL.len()]
}

const FOLDER: &str = include_str!("../assets/overview/folder.svg");

/// The sidebar symbol drawn in place of an outline icon, if there is one.
fn place_source(name: IconName) -> Option<&'static str> {
    Some(match name {
        IconName::House => include_str!("../assets/symbols/home.svg"),
        IconName::Images => include_str!("../assets/symbols/photos.svg"),
        IconName::ChartPie => include_str!("../assets/symbols/space-analyzer.svg"),
        IconName::DatabaseBackup => include_str!("../assets/symbols/backups.svg"),
        IconName::Monitor => include_str!("../assets/symbols/desktop.svg"),
        IconName::FileText => include_str!("../assets/symbols/documents.svg"),
        IconName::Download => include_str!("../assets/symbols/downloads.svg"),
        IconName::Image => include_str!("../assets/symbols/pictures.svg"),
        IconName::Music => include_str!("../assets/symbols/music.svg"),
        IconName::Clapperboard => include_str!("../assets/symbols/movies.svg"),
        IconName::Globe => include_str!("../assets/symbols/network.svg"),
        IconName::HardDrive | IconName::Usb => include_str!("../assets/symbols/drive.svg"),
        IconName::Settings => include_str!("../assets/symbols/settings.svg"),
        IconName::Radar => include_str!("../assets/symbols/nearby.svg"),
        IconName::Server => include_str!("../assets/symbols/server.svg"),
        IconName::Laptop => include_str!("../assets/symbols/laptop.svg"),
        IconName::Folder => FOLDER,
        _ => return None,
    })
}

/// Glyphs embossed on the folder's front face (64 grid, `{c}` is the color).
fn folder_glyph(name: &str) -> Option<&'static str> {
    let name = name.to_lowercase();
    Some(match name.as_str() {
        n if n == "applications" || n.starts_with("applications (") => {
            r#"<g fill="{c}"><rect x="23" y="28" width="7.5" height="7.5" rx="2"/><rect x="33.5" y="28" width="7.5" height="7.5" rx="2"/><rect x="23" y="38.5" width="7.5" height="7.5" rx="2"/><rect x="33.5" y="38.5" width="7.5" height="7.5" rx="3.75"/></g>"#
        }
        "desktop" => {
            r#"<g fill="none" stroke="{c}" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><rect x="21.5" y="28" width="21" height="13.5" rx="2"/><path d="M28 46.5h8M32 41.5v5"/></g>"#
        }
        "documents" => {
            r#"<g fill="none" stroke="{c}" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M26 27h8l5 5v13a1.5 1.5 0 0 1-1.5 1.5h-11.5A1.5 1.5 0 0 1 24.5 45V28.5A1.5 1.5 0 0 1 26 27Z"/><path d="M28.5 36h6M28.5 40.5h6"/></g>"#
        }
        "downloads" => {
            r#"<g fill="none" stroke="{c}" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="32" cy="37" r="9.5"/><path d="M32 31.5v10M27.8 37.6 32 41.8l4.2-4.2"/></g>"#
        }
        "library" => {
            r#"<g fill="{c}"><rect x="22.5" y="28" width="5" height="18" rx="1.2"/><rect x="29.5" y="30" width="5" height="16" rx="1.2"/><rect x="36.4" y="28.6" width="5" height="17.5" rx="1.2" transform="rotate(-14 38.9 37.35)"/></g>"#
        }
        "movies" | "videos" => {
            r#"<g fill="none" stroke="{c}" stroke-width="2.6" stroke-linejoin="round"><rect x="21.5" y="28.5" width="21" height="17" rx="2.5"/><path d="M27 28.5v17M37 28.5v17M21.5 37h5.5M37 37h5.5"/></g>"#
        }
        "music" => {
            r#"<path d="M28.5 43.5V31l11.5-3v12" fill="none" stroke="{c}" stroke-width="2.6" stroke-linejoin="round"/><g fill="{c}"><circle cx="25.6" cy="43.6" r="3.4"/><circle cx="37.1" cy="40.4" r="3.4"/></g>"#
        }
        "pictures" | "photos" => {
            r#"<rect x="21.5" y="28.5" width="21" height="17" rx="2.5" fill="none" stroke="{c}" stroke-width="2.6" stroke-linejoin="round"/><g fill="{c}"><path d="M23 44.5 29 38l4 4 3-2.5 5.5 5Z"/><circle cx="36.6" cy="33.4" r="2"/></g>"#
        }
        "public" => r#"<g fill="{c}"><circle cx="32" cy="32.5" r="4.6"/><path d="M23.5 46.5a8.5 8.5 0 0 1 17 0Z"/></g>"#,
        "games" => {
            r#"<g fill="none" stroke="{c}" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M26 31h12a5.5 5.5 0 0 1 5.3 6.9l-1.4 5a2.6 2.6 0 0 1-4.4 1.1L35 41.5h-6L26.5 44a2.6 2.6 0 0 1-4.4-1.1l-1.4-5A5.5 5.5 0 0 1 26 31Z"/><path d="M27 34.5v4M25 36.5h4"/></g><g fill="{c}"><circle cx="36.5" cy="35.4" r="1.4"/><circle cx="39" cy="38" r="1.4"/></g>"#
        }
        "code" | "coding" | "developer" | "dev" | "src" | "source" | "repos" => {
            r#"<path d="M26.5 31.5 21 37l5.5 5.5M37.5 31.5 43 37l-5.5 5.5M34 29.5l-4 15" fill="none" stroke="{c}" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>"#
        }
        "packages" => {
            r#"<g fill="none" stroke="{c}" stroke-width="2.6" stroke-linejoin="round"><path d="M32 27.5 41 32v10L32 46.5 23 42V32Z"/><path d="M23 32l9 4.5 9-4.5M32 36.5v10"/></g>"#
        }
        "projects" | "work" => {
            r#"<g fill="none" stroke="{c}" stroke-width="2.6" stroke-linejoin="round"><rect x="21.5" y="31.5" width="21" height="14" rx="2.5"/><path d="M28 31.5v-2a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M21.5 37.5h21"/></g>"#
        }
        "parallels" | "virtual machines" | "vms" => {
            r#"<g fill="{c}"><rect x="25" y="27.5" width="5" height="19" rx="1.4"/><rect x="34" y="27.5" width="5" height="19" rx="1.4"/></g>"#
        }
        "postman" | "api" => {
            r#"<path d="M33.5 27.5 26 38h6l-1.5 8.5L38 36h-6Z" fill="none" stroke="{c}" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>"#
        }
        "3d" | "models" => {
            r#"<g fill="{c}"><path d="M32 27 41 32l-9 5-9-5Z" opacity=".9"/><path d="M23 33.6l8 4.4v9.5l-8-4.5Z" opacity=".75"/><path d="M41 33.6 33 38v9.5l8-4.5Z" opacity=".55"/></g>"#
        }
        _ => return None,
    })
}

/// The folder artwork with `glyph` embossed: a light copy 1.2 px lower,
/// then the glyph itself in a deep blue.
fn embossed_folder(glyph: &str) -> String {
    let light = glyph.replace("{c}", "#e6f6ff");
    let dark = glyph.replace("{c}", "#0b3f78");
    FOLDER.replacen(
        "</svg>",
        &format!(r#"<g opacity=".45" transform="translate(0 1.2)">{light}</g><g opacity=".78">{dark}</g></svg>"#),
        1,
    )
}

/// Recolors every `#rrggbb` in `svg` to the accent hue, keeping each
/// color's luminance so the symbol's shading and detail survive.
fn tint(svg: &str, accent: Hsla) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(ix) = rest.find('#') {
        out.push_str(&rest[..ix]);
        let hex = rest.get(ix + 1..ix + 7).filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()));
        match hex.and_then(|h| u32::from_str_radix(h, 16).ok()) {
            Some(rgb) => {
                out.push_str(&tinted_hex(rgb, accent));
                rest = &rest[ix + 7..];
            }
            None => {
                out.push('#');
                rest = &rest[ix + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn tinted_hex(rgb: u32, accent: Hsla) -> String {
    let channel = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.;
    let luminance = 0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0);
    // Dark shades stay deep, whites become a pale wash of the accent.
    let l = 0.2 + 0.74 * luminance;
    let s = accent.s * (1. - 0.55 * luminance.powi(3));
    let c = gpui_kit::hsla(accent.h, s, l, 1.).to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    format!("#{:06x}", byte(c.r) << 16 | byte(c.g) << 8 | byte(c.b))
}

fn accent_key(accent: Hsla) -> u32 {
    let c = accent.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    byte(c.r) << 16 | byte(c.g) << 8 | byte(c.b)
}

/// Rasterized symbols, keyed by source, display edge and tint.
fn cached(svg: &str, edge: u32, accent: Option<Hsla>) -> Arc<Image> {
    type Key = (usize, u64, u32, Option<u32>);
    static CACHE: OnceLock<Mutex<HashMap<Key, Arc<Image>>>> = OnceLock::new();
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut hasher);
    let key = (svg.len(), hasher.finish(), edge, accent.map(accent_key));
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(key)
        .or_insert_with(|| {
            let svg = match accent {
                Some(accent) => tint(svg, accent),
                None => svg.to_string(),
            };
            // GPUI rasterizes SVGs at 2x their declared size; declaring the
            // display size keeps thin details instead of minifying a large texture.
            crate::art::hires_svg(&svg, edge, edge)
        })
        .clone()
}

fn symbol(svg: &str, size: f32, accent: Hsla) -> AnyElement {
    let tint = (icon_color() == IconColor::Tinted).then_some(accent);
    let edge = size.ceil().max(1.) as u32;
    img(cached(svg, edge, tint)).size(px(size)).flex_none().object_fit(ObjectFit::Contain).into_any_element()
}

/// A sidebar icon: the full-color (or tinted) symbol for `name` where one
/// exists, else the outline glyph tinted `color`.
pub fn place_icon(name: IconName, size: f32, color: Hsla, accent: Hsla) -> AnyElement {
    match place_source(name).filter(|_| icon_color() != IconColor::Outline) {
        // The symbols fill their artboard edge to edge; nudge them up a size
        // so they carry the same weight as the outline glyphs.
        Some(svg) => symbol(svg, size + 2., accent),
        None => crate::icons::icon(name).size(px(size)).text_color(color).into_any_element(),
    }
}

/// The folder symbol for a directory entry in a `size` × `size` box (file
/// list, columns, icon grid), or None when it keeps its regular icon
/// (outline mode, symlinks).
pub fn folder_icon(entry: &Entry, size: f32, accent: Hsla) -> Option<AnyElement> {
    if !entry.is_dir || entry.kind == EntryKind::Symlink || icon_color() == IconColor::Outline {
        return None;
    }
    Some(match folder_glyph(&entry.name) {
        Some(glyph) => {
            static EMBOSSED: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
            let svg = *EMBOSSED
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .entry(glyph)
                .or_insert_with(|| Box::leak(embossed_folder(glyph).into_boxed_str()));
            symbol(svg, size, accent)
        }
        None => symbol(FOLDER, size, accent),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tint_keeps_luminance_order_and_leaves_ids_alone() {
        let accent = gpui_kit::hsla(0.33, 0.7, 0.5, 1.);
        let svg = tint(r##"<a fill="#ffffff" href="#blue" stroke="#000000"/>"##, accent);
        assert!(svg.contains("href=\"#blue\""));
        let light = tinted_hex(0xffffff, accent);
        let dark = tinted_hex(0x000000, accent);
        assert!(svg.contains(&light) && svg.contains(&dark));
        assert!(u32::from_str_radix(&light[1..], 16).unwrap() > u32::from_str_radix(&dark[1..], 16).unwrap());
    }

    #[test]
    fn startup_styles_match_settings_defaults() {
        assert_eq!(rile_core::settings::IconSet::ALL[2], rile_core::settings::IconSet::default());
        assert_eq!(IconColor::ALL[1], IconColor::default());
    }

    #[test]
    fn well_known_folders_get_glyphs() {
        for name in ["Desktop", "Documents", "Downloads", "Applications (Parallels)", "music"] {
            assert!(folder_glyph(name).is_some(), "{name}");
        }
        assert!(folder_glyph("untitled folder").is_none());
        assert!(embossed_folder(folder_glyph("Desktop").unwrap()).trim_end().ends_with("</g></svg>"));
    }
}
