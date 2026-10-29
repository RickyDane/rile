//! The Nearby Devices page (`Location::Devices`): this computer and every
//! paired or discovered Rile device as a card with its storage and shared
//! folders. Drop files on a card to send them.

use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, menu::ContextMenuExt, menu::PopupMenuItem, v_flex};
use gpui_kit::{
    AnyElement, Context, Div, ExternalPaths, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use rile_remote::peer::DeviceKind;

use super::chrome::content_class;
use super::home::{PAGE_MAX_WIDTH, PAGE_TITLE_GAP, card, page_padding, page_title, page_title_top};
use crate::icons::icon;
use crate::pane::DraggedFiles;
use crate::theme::{Chrome, chrome};
use crate::views::kit;
use crate::workspace::Workspace;
use crate::workspace::nearby::DeviceRow;

/// Shared folders listed on a card before "+N more".
const MAX_CHIPS: usize = 5;

fn metric(label: &'static str, value: String, unit: Option<String>, detail: String, symbol: IconName, ch: &Chrome) -> AnyElement {
    card(ch)
        .flex_1()
        .min_w_0()
        .p(px(16.))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(6.))
        .child(h_flex().gap_2().child(icon(symbol).size(px(14.)).text_color(ch.accent)).child(text(label.to_uppercase(), 10., ch.text_muted)))
        .child(
            h_flex()
                .items_baseline()
                .gap(px(4.))
                .child(text(value, 24., ch.text_strong).font_weight(FontWeight::MEDIUM))
                .children(unit.map(|u| text(u, 11., ch.text_muted))),
        )
        .child(text(detail, 11., ch.text_faint))
        .into_any_element()
}

fn text(value: impl Into<SharedString>, size: f32, color: Hsla) -> Div {
    div().text_size(px(size)).text_color(color).child(value.into())
}

fn size(bytes: u64) -> String {
    humansize::format_size(bytes, humansize::DECIMAL)
}

pub fn kind_icon(kind: DeviceKind) -> IconName {
    match kind {
        DeviceKind::Laptop => IconName::Laptop,
        DeviceKind::Desktop => IconName::Monitor,
        DeviceKind::Server => IconName::Server,
    }
}

fn pill(label: &'static str, color: Hsla, dot: bool) -> Div {
    h_flex()
        .flex_none()
        .gap(px(5.))
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .bg(color.opacity(0.13))
        .when(dot, |d| d.child(div().size(px(6.)).rounded_full().bg(color)))
        .child(text(label, 10., color).font_weight(FontWeight::MEDIUM))
}

fn chip(label: impl Into<SharedString>, ic: Option<IconName>, ch: &Chrome) -> Div {
    h_flex()
        .gap(px(5.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(6.))
        .bg(ch.hover)
        .text_size(px(10.5))
        .text_color(ch.text_muted)
        .children(ic.map(|i| icon(i).size(px(11.)).text_color(ch.accent)))
        .child(label.into())
}

/// The storage bar: solid accent for used space.
fn storage_bar(used: u64, total: u64, ch: &Chrome) -> Div {
    let frac = if total == 0 { 0. } else { (used as f64 / total as f64).clamp(0., 1.) as f32 };
    let color = if frac >= 0.9 { gpui_kit::rgba(0xe5b567ff).into() } else { ch.accent };
    div().w_full().h(px(6.)).rounded_full().overflow_hidden().bg(ch.hover)
        .child(div().h_full().w(gpui_kit::relative(frac)).rounded_full().bg(color))
}

fn fact(label: &str, value: String, ch: &Chrome) -> AnyElement {
    v_flex().flex_1().min_w_0().p(px(10.)).rounded(px(6.)).bg(ch.hover)
        .gap(px(5.)).child(text(label.to_uppercase(), 9., ch.text_faint))
        .child(text(value, 13., ch.text_strong).truncate()).into_any_element()
}

fn node_symbol(symbol: IconName, ch: &Chrome) -> Div {
    div().relative().size(px(72.)).flex_none().flex().items_center().justify_center()
        .rounded_full().border_1().border_color(ch.accent.opacity(0.15)).bg(ch.accent.opacity(0.04))
        .child(div().absolute().size(px(54.)).rounded_full().border_1().border_color(ch.accent.opacity(0.3)))
        .child(icon(symbol).size(px(28.)).text_color(ch.accent))
}

struct CardData {
    key: String,
    icon: IconName,
    name: String,
    subtitle: String,
    pill: Option<(&'static str, Hsla, bool)>,
    storage: Option<(u64, u64)>,
    endpoint: String,
    version: String,
    trust: &'static str,
    cached: bool,
    shares_known: bool,
    shares: Vec<String>,
    /// Shown in place of shares when there are none.
    note: String,
}

fn device_card(data: CardData, ch: &Chrome) -> gpui_kit::Stateful<Div> {
    let (used, free, load) = storage_stats(data.storage);
    let storage = match data.storage {
        Some((used, total)) if total > 0 => v_flex()
            .gap(px(7.))
            .child(
                h_flex()
                    .justify_between()
                    .child(text(if data.cached { "STORAGE / LAST KNOWN" } else { "STORAGE / CAPACITY" }, 10., ch.text_muted))
                    .child(text(size(total), 11., ch.text_muted)),
            )
            .child(storage_bar(used, total, ch)),
        _ => v_flex()
            .gap(px(7.))
            .child(h_flex().justify_between().child(text("Storage", 11., ch.text_muted)).child(text("—", 11., ch.text_faint)))
            .child(storage_bar(0, 0, ch)),
    };
    let more = data.shares.len().saturating_sub(MAX_CHIPS);
    let chips = h_flex()
        .flex_wrap()
        .gap(px(6.))
        .min_h(px(22.))
        .children(data.shares.iter().take(MAX_CHIPS).map(|s| chip(s.clone(), Some(IconName::Folder), ch)))
        .when(more > 0, |d| d.child(chip(format!("+{more} more"), None, ch)));
    card(ch)
        .id(SharedString::from(format!("device-{}", data.key)))
        .debug_selector({
            let key = data.key.clone();
            move || format!("device-{key}")
        })
        .flex_1()
        .min_w_0()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(18.))
        .cursor_pointer()
        .hover(|s| s.border_color(ch.accent.opacity(0.45)))
        .child(
            h_flex()
                .flex_wrap()
                .gap(px(12.))
                .items_start()
                .child(node_symbol(data.icon, ch))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(3.))
                        .child(text("NODE / ".to_owned() + &data.key, 9., ch.text_faint).truncate())
                        .child(text(data.name.clone(), 16., ch.text_strong).font_weight(FontWeight::SEMIBOLD).truncate())
                        .child(text(data.subtitle.clone(), 11., ch.text_muted).truncate()),
                )
                .children(data.pill.map(|(l, c, dot)| pill(l, c, dot))),
        )
        .child(storage)
        .child(grid(vec![fact("Used", used, ch), fact("Available", free, ch), fact("Utilization", load, ch)], 3, 6.))
        .child(
            v_flex().gap(px(8.))
                .child(h_flex().justify_between().gap_2().child(text("ENDPOINT", 9., ch.text_faint)).child(text(data.endpoint, 11., ch.text_muted).truncate()))
                .child(h_flex().justify_between().gap_2().child(text("RILE VERSION", 9., ch.text_faint)).child(text(data.version, 11., ch.text_muted)))
                .child(h_flex().justify_between().gap_2().child(text("ACCESS", 9., ch.text_faint)).child(text(data.trust, 11., ch.text_muted))),
        )
        .child(if data.shares.is_empty() {
            h_flex().gap(px(6.)).items_start()
                .child(icon(IconName::Folder).size(px(12.)).flex_none().text_color(ch.text_faint))
                .child(text(data.note, 10.5, ch.text_faint).flex_1().min_w_0())
                .into_any_element()
        } else {
            v_flex().gap(px(8.)).pt(px(10.)).border_t_1().border_color(ch.hairline)
                .child(h_flex().justify_between().child(text("SHARED DIRECTORIES", 9., ch.text_faint))
                    .child(text(if data.shares_known { format!("{:02}", data.shares.len()) } else { "—".into() }, 11., ch.text_muted)))
                .child(chips).into_any_element()
        })
}

fn storage_stats(storage: Option<(u64, u64)>) -> (String, String, String) {
    match storage {
        Some((used, total)) if total > 0 => (
            size(used), size(total.saturating_sub(used)),
            format!("{:.1}%", (used as f64 / total as f64 * 100.).clamp(0., 100.)),
        ),
        _ => ("—".into(), "—".into(), "—".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{size, storage_stats};

    #[test]
    fn machine_storage_statistics_use_real_capacity() {
        assert_eq!(storage_stats(Some((250, 1000))), (size(250), size(750), "25.0%".into()));
        assert_eq!(storage_stats(Some((1200, 1000))), (size(1200), size(0), "100.0%".into()));
    }

    #[test]
    fn unknown_capacity_is_not_reported_as_empty_storage() {
        for value in [None, Some((0, 0))] {
            assert_eq!(storage_stats(value), ("—".into(), "—".into(), "—".into()));
        }
    }
}

fn subtitle(model: &str, os: &str) -> String {
    match (model.is_empty(), os.is_empty()) {
        (false, false) => format!("{model} · {os}"),
        (false, true) => model.to_string(),
        (true, false) => os.to_string(),
        (true, true) => String::new(),
    }
}

/// Rows of `per_row` equally wide cells.
fn grid(cells: Vec<AnyElement>, per_row: usize, gap: f32) -> AnyElement {
    let mut rows = Vec::new();
    let mut iter = cells.into_iter().peekable();
    while iter.peek().is_some() {
        let row: Vec<AnyElement> = iter.by_ref().take(per_row).collect();
        let missing = per_row - row.len();
        rows.push(
            h_flex()
                .gap(px(gap))
                .items_stretch()
                .children(row.into_iter().map(|c| div().flex_1().min_w_0().flex().child(c)))
                .children((0..missing).map(|_| div().flex_1()))
                .into_any_element(),
        );
    }
    v_flex().gap(px(gap)).children(rows).into_any_element()
}

fn row_card(row: &DeviceRow, ws: &Workspace, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let warn: Hsla = gpui_kit::rgba(0xe5b567ff).into();
    let pairing = ws.nearby.pairing.contains(&row.id);
    let pill = if pairing {
        Some(("Pairing…", ch.text_muted, false))
    } else if !row.paired {
        Some(("Not paired", warn, false))
    } else if row.forgot_us {
        Some(("Unpaired there", warn, false))
    } else if row.online.is_some() {
        Some(("LAN", ch.accent, true))
    } else {
        Some(("Offline", ch.text_faint, true))
    };
    let details = row.details.as_ref();
    let data = CardData {
        key: row.id.chars().take(12).collect(),
        icon: kind_icon(row.kind),
        name: row.name.clone(),
        subtitle: subtitle(&row.model, &row.os),
        pill,
        storage: details.map(|d| (d.storage_used, d.storage_total)),
        endpoint: row.online.as_ref().map(|(host, port)| format!("{host}:{port}")).unwrap_or_else(|| "Offline".into()),
        version: details.map(|d| d.version.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| "—".into()),
        trust: if row.forgot_us || !row.paired { "Pairing required" } else { "Paired · read-only" },
        cached: row.online.is_none(),
        shares_known: details.is_some(),
        shares: details.map(|d| d.shares.clone()).unwrap_or_default(),
        note: if !row.paired {
            "Click to pair. Both screens show a code to compare.".into()
        } else if row.online.is_none() {
            "Not on the network right now".into()
        } else if details.is_some() {
            "Shares no folders · drop files here to send".into()
        } else {
            "Connecting…".into()
        },
    };
    let (id, id2, id3) = (row.id.clone(), row.id.clone(), row.id.clone());
    let paired = row.paired;
    let online = row.online.is_some();
    let weak = cx.entity().downgrade();
    let shares = row.details.as_ref().map(|d| d.shares.clone()).unwrap_or_default();
    let mut el = device_card(data, ch)
        .when(!online, |d| d.opacity(0.6))
        .on_click(cx.listener(move |ws, _, w, cx| ws.open_device(&id, None, w, cx)));
    if online && paired {
        let drop_bg = ch.accent.opacity(0.10);
        let accent = ch.accent;
        let id_ext = id2.clone();
        el = el
            .drag_over::<DraggedFiles>(move |s, _, _, _| s.border_color(accent).bg(drop_bg))
            .drag_over::<ExternalPaths>(move |s, _, _, _| s.border_color(accent).bg(drop_bg))
            .on_drop(cx.listener(move |ws, d: &DraggedFiles, w, cx| {
                if d.conn.is_some() {
                    return;
                }
                let paths = d.resolve(cx);
                ws.send_to_device(&id2, paths, w, cx);
            }))
            .on_drop(cx.listener(move |ws, ext: &ExternalPaths, w, cx| ws.send_to_device(&id_ext, ext.paths().to_vec(), w, cx)));
    }
    el.context_menu(move |menu, _, _| {
        let action = |label: String, f: std::rc::Rc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>| {
            let weak = weak.clone();
            PopupMenuItem::new(label).on_click(move |_, w, cx| {
                let f = f.clone();
                weak.update(cx, |ws, cx| f(ws, w, cx)).ok();
            })
        };
        let id = id3.clone();
        let mut menu = menu;
        if !paired {
            let id = id.clone();
            return menu.item(action("Pair…".into(), std::rc::Rc::new(move |ws, w, cx| ws.pair_with(&id, w, cx))));
        }
        if online {
            let open = id.clone();
            menu = menu.item(action("Open".into(), std::rc::Rc::new(move |ws, w, cx| ws.open_device(&open, None, w, cx))));
            for share in &shares {
                let (open, share) = (id.clone(), share.clone());
                menu = menu.item(action(format!("Open “{share}”"), std::rc::Rc::new(move |ws, w, cx| ws.open_device(&open, Some(&share), w, cx))));
            }
            let send = id.clone();
            menu = menu.separator().item(action("Send Files…".into(), std::rc::Rc::new(move |ws, w, cx| ws.pick_and_send(&send, w, cx))));
        }
        let forget = id.clone();
        menu.separator().item(action("Unpair…".into(), std::rc::Rc::new(move |ws, w, cx| ws.unpair(&forget, w, cx))))
    })
    .into_any_element()
}

fn this_device_card(ws: &Workspace, ch: &Chrome, cx: &mut Context<Workspace>) -> AnyElement {
    let local = &ws.nearby.local.info;
    let facts = ws.nearby.facts.clone().unwrap_or_default();
    let name = if local.name.is_empty() { facts.name.clone() } else { local.name.clone() };
    let data = CardData {
        key: "this".into(),
        icon: kind_icon(facts.kind),
        name: if name.is_empty() { "This computer".into() } else { name },
        subtitle: subtitle(&facts.model, &facts.os),
        pill: Some(("This device", ch.success, true)),
        storage: Some((local.storage_used, local.storage_total)),
        endpoint: "Local machine".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        trust: "Owner · read-only shares",
        cached: false,
        shares_known: true,
        shares: ws.settings.nearby.shares.iter().map(|s| s.name.clone()).collect(),
        note: "Not sharing any folders · right-click a folder to share it".into(),
    };
    device_card(data, ch)
        .on_click(cx.listener(|ws, _, w, cx| {
            crate::dialogs::open_nearby_settings(cx.entity(), ws.settings.clone(), w, cx)
        }))
        .into_any_element()
}

fn empty_hint(title: &str, body: &str, ch: &Chrome) -> AnyElement {
    card(ch)
        .p(px(22.))
        .flex()
        .gap(px(16.))
        .items_center()
        .child(
            div()
                .size(px(48.))
                .flex_none()
                .rounded(px(12.))
                .bg(ch.accent.opacity(0.12))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::Radar).size(px(24.)).text_color(ch.accent)),
        )
        .child(
            v_flex()
                .gap(px(6.))
                .child(text(title.to_string(), 14., ch.text_strong).font_weight(FontWeight::SEMIBOLD))
                .child(text(body.to_string(), 12., ch.text_muted).max_w(px(620.))),
        )
        .into_any_element()
}

pub fn render(ws: &mut Workspace, slot: usize, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let ch = chrome(cx);
    let width = ws.slot_width(slot, window);
    let class = content_class(width);
    let (pad_x, _) = page_padding(class);
    let rows = ws.nearby.rows();
    let paired = rows.iter().filter(|r| r.paired).count();
    let online_paired: Vec<&DeviceRow> = rows.iter().filter(|r| r.paired && r.online.is_some()).collect();
    let unpaired = rows.iter().filter(|r| !r.paired).count();
    let shared = ws.settings.nearby.shares.len();
    let local = &ws.nearby.local.info;
    let total_storage: u64 = local.storage_total + online_paired.iter().filter_map(|r| r.details.as_ref()).map(|d| d.storage_total).sum::<u64>();
    let used_storage: u64 = local.storage_used + online_paired.iter().filter_map(|r| r.details.as_ref()).map(|d| d.storage_used).sum::<u64>();
    let remote_shares: usize = online_paired.iter().filter_map(|r| r.details.as_ref()).map(|d| d.shares.len()).sum();

    let title = h_flex()
        .mb(px(PAGE_TITLE_GAP))
        .justify_between()
        .gap_2()
        .flex_wrap()
        .child(v_flex().gap(px(6.)).child(text("LOCAL NETWORK / DEVICE CONSOLE", 10., ch.accent)).child(page_title("Nearby", class, &ch)))
        .child(
            kit::button("devices-settings", "Sharing Settings", kit::Tone::Secondary, true, cx).on_click(cx.listener(|ws, _, w, cx| {
                crate::dialogs::open_nearby_settings(cx.entity(), ws.settings.clone(), w, cx)
            })),
        );

    let content_width = width.min(PAGE_MAX_WIDTH) - pad_x * 2.;
    let metric_cols = if content_width >= 670. { 4 } else if content_width >= 320. { 2 } else { 1 };
    let total_value = humansize::format_size(total_storage, humansize::DECIMAL);
    let (value, unit) = match total_value.split_once(' ') {
        Some((v, u)) => (v.to_string(), Some(u.to_string())),
        None => (total_value, None),
    };
    let metrics = grid(
        vec![
            metric("Storage pool", value, unit, format!("{} available · online devices", size(total_storage.saturating_sub(used_storage))), IconName::Server, &ch),
            metric(
                "Paired devices",
                paired.to_string(),
                None,
                format!("{} online now", online_paired.len()),
                IconName::Monitor,
                &ch,
            ),
            metric("Shared folders", (shared + remote_shares).to_string(), None, "Read-only network access".into(), IconName::Folder, &ch),
            metric("Discovered", unpaired.to_string(), None, "Awaiting trusted pairing".into(), IconName::Radar, &ch),
        ],
        metric_cols,
        10.,
    );

    let card_cols = if content_width >= 880. { 2 } else { 1 };
    let discovery_active = ws.settings.nearby.enabled && ws.nearby.error.is_none();
    let network_status = if !ws.settings.nearby.enabled { "DISCOVERY OFF" } else if ws.nearby.error.is_some() { "DISCOVERY UNAVAILABLE" } else { "DISCOVERY ACTIVE" };
    let mut page = v_flex().gap(px(18.))
        .child(
            h_flex().flex_wrap().gap(px(12.)).p(px(12.)).rounded(px(8.)).border_1().border_color(ch.accent.opacity(0.18)).bg(ch.accent.opacity(0.04))
                .child(icon(IconName::Radar).size(px(18.)).text_color(ch.accent))
                .child(pill(network_status, if discovery_active { ch.success } else { ch.text_faint }, true))
                .child(text("DIRECT LAN  /  ENCRYPTED TRANSPORT  /  NO CLOUD RELAY", 10., ch.text_muted)),
        )
        .child(div().w_full().mb(px(6.)).child(metrics))
        .child(h_flex().justify_between().flex_wrap().gap_2()
            .child(text("MACHINE INVENTORY", 11., ch.text_muted).font_weight(FontWeight::SEMIBOLD))
            .child(text(format!("{:02} NODES  /  {} REMOTE ONLINE", rows.len() + 1, rows.iter().filter(|r| r.online.is_some()).count()), 10., ch.text_faint)));
    if !ws.settings.nearby.enabled {
        page = page.child(empty_hint(
            "Nearby Devices is off",
            "Turn it on in Settings → Nearby to find other computers running Rile on your network, browse the folders they share and send them files.",
            &ch,
        ));
    } else if let Some(err) = &ws.nearby.error {
        page = page.child(empty_hint("Nearby Devices couldn't start", err, &ch));
    } else {
        let mut cards = vec![this_device_card(ws, &ch, cx)];
        for row in &rows {
            cards.push(row_card(row, ws, &ch, cx));
        }
        if rows.is_empty() {
            cards.push(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .justify_center()
                    .p(px(24.))
                    .gap(px(12.))
                    .text_center()
                    .child(icon(IconName::Radar).size(px(36.)).text_color(ch.text_faint))
                    .child(text("No remote nodes detected", 14., ch.text_muted).font_weight(FontWeight::SEMIBOLD))
                    .child(text(
                        "Open Rile on another computer on your local network, then verify the pairing code on both screens.",
                        12., ch.text_faint,
                    ).max_w(px(300.)))
                    .into_any_element(),
            );
        }
        page = page.child(grid(cards, card_cols, 14.));
    }
    page = page.child(
        h_flex()
            .gap_2()
            .items_start()
            .child(icon(IconName::Info).size(px(12.)).flex_none().mt(px(2.)).text_color(ch.text_faint))
            .child(text(
                "Devices talk directly over your local network, encrypted. Paired devices can browse what you share (read-only) and offer you files; you always decide whether to accept.",
                11.,
                ch.text_faint,
            ).flex_1().min_w_0()),
    );
    div()
        .id(("devices-page", slot))
        .debug_selector(|| "devices-page".into())
        .size_full()
        .bg(ch.browser)
        .overflow_y_scroll()
        .child(v_flex().w_full().max_w(px(PAGE_MAX_WIDTH)).mx_auto().px(px(pad_x)).pt(px(page_title_top(class))).pb(px(28.)).child(title).child(page))
        .into_any_element()
}
