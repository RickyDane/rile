# Rile design guide

This guide captures the proposed visual language in the [design-system mockup](../mockups/design-system.html). It is a direction for review, not a claim that the native application or plugins already implement it. Use the same conventions across the explorer, dialogs, tabs, and plugin surfaces.

## Principles

- **Content first:** use quiet, neutral chrome so files and information remain the focus.
- **One vocabulary:** the same action should look and behave the same wherever it appears, including inside plugins.
- **Clear hierarchy:** reserve accent color for the main action; use neutral controls for supporting actions.
- **Compact, not cramped:** use the compact control size in dense rows, not as the default everywhere.
- **Accessible by default:** pair icons with labels where space permits, expose accessible names for icon-only controls, and make keyboard focus visible.

## Foundations

The mockup uses a dark palette and system UI typography. Values below are starting points for implementation; validate contrast and native rendering before shipping.

| Token | Value | Purpose |
| --- | --- | --- |
| Canvas | `#1d1e21` | Main application background |
| Surface | `#282a2e` | Cards and grouped content |
| Inset | `#202125` | Tab containers and embedded panels |
| Control | `#383b41` | Default button background |
| Text | `#f1f2f5` | Primary content |
| Muted text | `#969ba5` | Supporting descriptions |
| Accent | `#8bbfff` | Emphasis, focus, and visual indicators |
| Primary button | `#86baf5` | Main action, with dark `#15283d` text |
| Destructive button | `#533236` | Destructive action, with `#ffb4b4` text |

Use subtle translucent borders (`#ffffff12` for surfaces, `#ffffff18` for controls). The mockup uses 12 px card corners, 7–8 px control corners, and 5 px selected-tab corners. Maintain a consistent spacing rhythm: 4 px within segmented controls, 8–10 px between related elements, and 16–24 px between groups.

Use the platform system sans-serif. The mockup uses 14 px body text, 15 px section headings, 12 px control labels, and 11–12 px supporting text. Headings are semibold; button labels are semibold rather than all caps.

## Buttons

Use one visual family across the application and plugin host. Default buttons are 34 px high with 8 px vertical and 13 px horizontal padding; compact buttons are 27 px high with 5 px vertical and 9 px horizontal padding. Icon-only buttons are 34 px square. Keep labels short and action-oriented.

| Variant | Use | Example |
| --- | --- | --- |
| Primary | The most important action in a local context; generally one per group | Choose folder |
| Secondary | Ordinary supporting actions | New folder, Copy path |
| Ghost | Low-emphasis or dismissive actions | Cancel, Reveal |
| Destructive | An action that removes or irreversibly changes data | Remove |
| Icon-only | Familiar, space-constrained actions; always provide an accessible name | More options |
| Disabled | Unavailable actions; explain why where practical | Unavailable |

Provide hover, pressed, disabled, and visible keyboard-focus states consistently. The mockup shows a lighter hover and a 2 px accent focus outline with a 2 px offset. Disabled controls use reduced opacity and must not respond to activation. Do not rely on color alone to communicate destructive intent; use an explicit label and confirmation where appropriate.

## Tabs and navigation

Place related tabs in a dark inset container with 4 px padding and a modest gap. The selected tab uses a raised neutral fill (`#474b52`) and bright text; inactive tabs remain transparent with muted text. Use the same selection language for plugin tabs and app navigation. Labels should remain visible alongside icons. Implement keyboard navigation and expose selected state semantically; focus and selection are distinct states.

Sidebar navigation is quieter than the content area. A selected item receives a subtle neutral background, not the primary-action blue. Group related destinations under small, muted section labels.

## Icons

Use the shared explorer vocabulary in [`mockups/icons.js`](../mockups/icons.js), rather than mixing unrelated symbols or icon styles. Icons are 16 px, outlined, and drawn with a consistent 1.7 px stroke, rounded caps and joins. Keep them aligned with text and leave approximately 6 px between an icon and its button or tab label. Navigation icons may use a muted blue-gray (`#a2bcd9`).

Common concepts include home, folder, new folder, search, recent, favorite, file, storage, grid, list, columns, sort, download, and more. Choose an icon for its meaning, not decoration; retain a text label when meaning could be ambiguous. Decorative icons should be hidden from assistive technology, while icon-only controls need an accessible name.

### AI actions

Mark AI features (intelligent search, describing photos, assistant actions) with the system Apple Intelligence symbol in its multicolor gradient, not a generic sparkle or magic-wand icon. In code, use `icons::apple_intelligence`, which renders the native SF Symbol and falls back to the sparkle icon only where the symbol is unavailable (non-macOS, or before it loads). Size it like any other icon (16 px in buttons, 6 px from the label) and keep the action's text label, such as **Describe All**. Menus and other places that accept only SVG icons use `icons::ai_icon`, the same symbol as a single-color glyph tinted like the surrounding icons. Dialog headers use `kit::dialog_with_glyph` with the symbol in the icon tile.

## Plugin surfaces

Plugins should feel like part of Rile, not separate applications. Use host-provided controls and the same button hierarchy, icons, spacing, tab selection, typography, and focus states. A plugin label may distinguish the source of a page without changing its interaction language. In Space Analyzer, for example, **Choose folder** is primary, **Open folder** and **Copy path** are supporting actions, and Overview, Files, and Cleanup share the standard tab treatment.

Avoid implying that illustrative values are live data. Destructive plugin actions should follow the same safeguards and wording as native actions. This guide describes the desired presentation; any required plugin API or host-renderer changes remain implementation work.

## Review checklist

- Is the primary action clear, with secondary and destructive actions visually distinct?
- Are controls consistent in size, spacing, icon style, and interaction states across app and plugins?
- Are text and control boundaries legible against their surfaces, including disabled and focused states?
- Can every action be reached and understood with a keyboard and assistive technology?
- Are icon-only actions named, and are selected tabs distinguishable without relying solely on color?
- Does the interface remain usable at narrow widths and with longer labels?

For the visual reference, serve `mockups/` locally and open `design-system.html`; see the [mockup README](../mockups/README.md) for the server command.
