//! Right-hand inspector: preview stage, identity, quick actions and the
//! selection's real metadata (sizes, dates, image dimensions, access).

use std::time::SystemTime;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    Selectable, Sizable, h_flex,
    button::{Button, ButtonVariants},
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ObjectFit, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, Window, div, img, linear_color_stop, linear_gradient,
    prelude::FluentBuilder, px,
};
use rile_core::Entry;
use rile_core::history::Location;
use rile_services::preview::Preview;

use super::preview::Loaded;
use super::{Workspace, display_name, item_count};
use crate::actions::*;
use crate::theme::{Chrome, chrome};
use crate::views::chrome::{Width, kbd, width_class};

pub fn width(window: &Window) -> f32 {
    match width_class(window) {
        Width::Phone => 270.,
        Width::Narrow => 220.,
        Width::Compact => 241.,
        Width::Regular => 276.,
        Width::Wide => 300.,
    }
}

fn date(t: Option<SystemTime>) -> String {
    t.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%b %-d, %Y").to_string()).unwrap_or_else(|| "—".into())
}

fn section(title: &'static str, icon: IconName, rows: Vec<(&'static str, String)>, ch: &Chrome) -> AnyElement {
    v_flex()
        .mx(px(20.))
        .py(px(18.))
        .border_t_1()
        .border_color(ch.hairline)
        .child(
            h_flex()
                .gap(px(8.))
                .mb(px(5.))
                .text_size(px(9.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ch.text_faint)
                .child(crate::icons::icon(icon).size(px(13.)).text_color(ch.accent))
                .child(title.to_uppercase()),
        )
        .children(rows.into_iter().map(|(k, v)| {
            h_flex()
                .mt(px(12.))
                .gap(px(12.))
                .justify_between()
                .items_start()
                .text_size(px(10.))
                .child(div().flex_none().text_color(ch.text_faint).child(k))
                .child(div().min_w_0().text_right().text_color(ch.text).child(v))
        }))
        .into_any_element()
}

impl Workspace {
    /// The entry the inspector describes (the previewed selection).
    fn inspected_entry(&self, cx: &gpui_kit::App) -> Option<Entry> {
        let target = self.preview.target.as_ref()?;
        let pane = self.pane().read(cx);
        pane.cursor_entry()
            .filter(|e| &e.path == target)
            .or_else(|| pane.selected_entries().into_iter().find(|e| &e.path == target))
            .cloned()
    }

    pub fn render_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ch = chrome(cx);
        let w = width(window);
        let entry = self.inspected_entry(cx);
        let pane = self.pane().read(cx);
        let pane_conn = pane.conn();
        let local = pane_conn.is_none();
        let location = pane.location.clone();
        let total = pane.total_count();
        let thumbnails = self.settings.show_thumbnails;

        let header = h_flex()
            .h(px(55.))
            .flex_none()
            .px(px(21.))
            .justify_between()
            .text_size(px(10.))
            .text_color(ch.text_faint)
            .child("Inspector")
            .child(
                Button::new("close-inspector").cursor_pointer()
                    .ghost()
                    .xsmall()
                    .icon(crate::icons::icon(IconName::X).size(px(13.)))
                    .tooltip(format!("Hide inspector ({})", crate::views::chrome::shortcut("shift-mod-p")))
                    .on_click(cx.listener(|this, _, w, cx| this.toggle_preview_panel(&TogglePreviewPanel, w, cx))),
            );

        let stage = |content: AnyElement, flush: bool| {
            div()
                .mx(px(if w < 250. { 17. } else { 22. }))
                .mt(px(3.))
                .h(px(if w < 250. { 164. } else if w > 290. { 220. } else { 190. }))
                .flex_none()
                .rounded(px(9.))
                .overflow_hidden()
                .border_1()
                .border_color(ch.hairline)
                .bg(linear_gradient(180., linear_color_stop(ch.stage_inner, 0.), linear_color_stop(ch.stage_outer, 1.)))
                .flex()
                .items_center()
                .justify_center()
                .when(!flush, |d| d.p(px(10.)))
                .child(content)
                .into_any_element()
        };

        let Some(entry) = entry else {
            // Nothing selected: describe the current location.
            let (title, detail, art) = match &location {
                Location::Home => ("Home".to_string(), "Select a file to see its details".to_string(), crate::art::this_computer()),
                other => (crate::workspace::tab_title(other), item_count(total), crate::art::Art::Folder),
            };
            return v_flex()
                .id("inspector")
                .w(px(w))
                .h_full()
                .flex_none()
                .bg(ch.inspector)
                .border_l_1()
                .border_color(ch.hairline)
                .child(header)
                .child(stage(crate::art::art(art, 96.), false))
                .child(
                    v_flex()
                        .items_center()
                        .px(px(18.))
                        .pt(px(19.))
                        .gap(px(7.))
                        .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(ch.text_strong).child(title))
                        .child(div().text_size(px(10.)).text_color(ch.text_faint).text_center().child(detail)),
                )
                .into_any_element();
        };

        // Only use results read from this pane's filesystem (see `target_conn`).
        let same_fs = self.preview.loaded_conn == pane_conn;
        let info = self.preview.info.as_ref().filter(|(p, _)| same_fs && *p == entry.path).map(|(_, i)| i.clone());
        let loaded = self
            .preview
            .loaded
            .as_ref()
            .filter(|(p, ..)| same_fs && *p == entry.path)
            .map(|(_, l, lines)| (l.clone(), lines.clone()));
        let props = info.as_ref().and_then(|i| i.props.clone());
        let dims = info.as_ref().and_then(|i| i.dimensions);
        let kind = props.as_ref().map(|p| p.kind_label.clone()).unwrap_or_else(|| entry.kind_label());
        let size_label = if entry.is_dir {
            match loaded.as_ref().map(|(l, _)| &**l) {
                Some(Loaded::Preview(Preview::Directory { total, .. })) => item_count(*total),
                _ => "—".into(),
            }
        } else {
            humansize::format_size(entry.size, humansize::DECIMAL)
        };

        let stage_content: AnyElement = match loaded.as_ref().map(|(l, lines)| (&**l, lines.clone())) {
            Some((Loaded::Preview(Preview::Image(p)), _)) if local => {
                let thumb = if entry.extension() == "svg" {
                    Some(gpui_kit::ImageSource::from(p.clone()))
                } else {
                    crate::art::preview_image(p, entry.modified, entry.size, w, cx).map(Into::into)
                };
                match thumb {
                    Some(source) => img(source).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                    None => crate::art::file_object(&entry, 120., false, local, cx),
                }
            }
            Some((Loaded::Pdf { .. }, _)) if local && !self.preview.pdf_pages.is_empty() => div()
                .size_full()
                .p(px(10.))
                .flex()
                .items_center()
                .justify_center()
                .child(img(self.preview.pdf_pages[0].clone()).size_full().object_fit(ObjectFit::Contain).shadow_md())
                .into_any_element(),
            Some((Loaded::Preview(Preview::Text { .. }) | Loaded::Plugin(_), lines)) if !lines.is_empty() => {
                let mono = gpui_kit::component::ActiveTheme::theme(&**cx).mono_font_family.clone();
                v_flex()
                    .size_full()
                    .p(px(12.))
                    .overflow_hidden()
                    .font_family(mono)
                    .text_size(px(9.))
                    .line_height(px(14.))
                    .text_color(ch.text_muted)
                    .children(lines.iter().take(14).map(|l| div().whitespace_nowrap().child(l.clone())))
                    .into_any_element()
            }
            _ => crate::art::file_object(&entry, 120., thumbnails, local, cx),
        };
        let flush = matches!(loaded.as_ref().map(|(l, _)| &**l), Some(Loaded::Preview(Preview::Image(_)) | Loaded::Pdf { .. })) && local;

        let favorite = self.settings.favorites.contains(&entry.path);
        let round = |id: &'static str, icon: IconName, tip: &'static str, active: bool| {
            Button::new(id).cursor_pointer()
                .ghost()
                .xsmall()
                .icon(crate::icons::icon(icon).size(px(13.)))
                .tooltip(tip)
                .size(px(29.))
                .rounded_full()
                .border_1()
                .border_color(if active { ch.accent.opacity(0.4) } else { ch.hairline })
                .when(active, |b| b.bg(ch.accent_soft).text_color(ch.accent))
                .selected(active)
        };
        let path = entry.path.clone();
        let is_dir = entry.is_dir;
        let actions = h_flex()
            .justify_center()
            .gap(px(9.))
            .mt(px(17.))
            .mb(px(23.))
            .when(local && is_dir, |d| {
                let path = path.clone();
                d.child(
                    round("inspector-fav", IconName::Heart, if favorite { "Remove from Favorites" } else { "Add to Favorites" }, favorite)
                        .on_click(cx.listener(move |this, _, w, cx| this.toggle_favorite_path(path.clone(), w, cx))),
                )
            })
            .child(
                round("inspector-open", IconName::Maximize2, if is_dir { "Open folder" } else { crate::platform_text::preview() }, false).on_click(
                    cx.listener(move |this, _, w, cx| {
                        if is_dir {
                            this.pane().update(cx, |p, cx| p.open_selected(cx));
                        } else {
                            this.toggle_quicklook(&ToggleQuickLook, w, cx);
                        }
                    }),
                ),
            );

        let ext = entry.extension();
        let parent = entry.path.parent().map(display_name).unwrap_or_default();
        let mut details = vec![(if is_dir { "Contains" } else { "Size" }, size_label.clone())];
        if !is_dir && !ext.is_empty() {
            details.push(("Extension", ext.clone()));
        }
        details.push(("Kind", kind.clone()));
        details.push(("Location", parent));

        let mut sections = vec![section("Details", IconName::Info, details, &ch)];
        if let Some(p) = &props {
            sections.push(section(
                "Dates",
                IconName::Calendar,
                vec![("Created", date(p.created)), ("Modified", date(p.modified)), ("Accessed", date(p.accessed))],
                &ch,
            ));
        } else {
            sections.push(section("Dates", IconName::Calendar, vec![("Modified", date(entry.modified))], &ch));
        }
        if let Some((w, h)) = dims {
            sections.push(section("Image", IconName::Image, vec![("Dimensions", format!("{w} × {h}"))], &ch));
        }
        if let Some(p) = &props {
            let mut rows = Vec::new();
            if let Some(owner) = &p.owner {
                rows.push(("Owner", owner.clone()));
            }
            if let Some(perm) = &p.permissions {
                rows.push(("Permissions", perm.clone()));
            }
            if !rows.is_empty() {
                sections.push(section("Access", IconName::Tag, rows, &ch));
            }
        }

        let open_label: SharedString = if is_dir { "Open folder".into() } else { crate::platform_text::preview().into() };
        let open_button = h_flex()
            .id("inspector-primary")
            .mx(px(20.))
            .mt(px(4.))
            .mb(px(22.))
            .p(px(9.))
            .gap(px(8.))
            .justify_center()
            .rounded(px(6.))
            .border_1()
            .border_color(ch.hairline)
            .bg(ch.card_bottom)
            .cursor_pointer()
            .hover(|s| s.bg(ch.hover_strong))
            .text_size(px(10.))
            .text_color(ch.accent)
            .child(crate::icons::icon(if is_dir { IconName::Folder } else { IconName::Maximize2 }).size(px(13.)))
            .child(open_label)
            .child(kbd(if is_dir { "↵" } else { "Space" }, &ch))
            .on_click(cx.listener(move |this, _, w, cx| {
                if is_dir {
                    this.pane().update(cx, |p, cx| p.open_selected(cx));
                } else {
                    this.toggle_quicklook(&ToggleQuickLook, w, cx);
                }
            }));

        v_flex()
            .id("inspector")
            .w(px(w))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .bg(ch.inspector)
            .border_l_1()
            .border_color(ch.hairline)
            .child(header)
            .child(stage(stage_content, flush))
            .child(
                v_flex()
                    .items_center()
                    .px(px(18.))
                    .pt(px(19.))
                    .gap(px(7.))
                    .child(
                        div()
                            .text_center()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ch.text_strong)
                            .child(SharedString::from(entry.name.to_string())),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(ch.text_faint)
                            .child(if is_dir { kind.clone() } else { format!("{kind} · {size_label}") }),
                    ),
            )
            .child(actions)
            .children(sections)
            .child(open_button)
            .into_any_element()
    }
}
