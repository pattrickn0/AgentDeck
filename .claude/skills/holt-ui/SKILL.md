---
name: holt-ui
description: Visual and code rules for the Holt interface (egui, crates/app/src/ui and theme.rs). Use before creating or changing anything in the app's interface — sidebar, title bar, split view, right bar, menus, modals, icons, colors, spacing, user-facing text — and to check the result with a screenshot.
---

# Holt interface

The app is plain egui, mostly without ready-made widgets: each component is **painted by hand** with the `Painter`
and gets interaction through `ui.interact`/`clickable`. Follow what already exists before inventing.

Supporting reference (installed in the project): `rust-desktop-applications` (`references/native-gui-frameworks.md`
for egui). Where it disagrees with these rules, these win.

## Product principles

1. **Agents are the center.** What matters at a glance: which agents exist, which one is working, which one finished
   unseen, who answers whom. Files, git and settings are secondary.
2. **State visible without clicking.** Working = halo pulsing in the agent's color; finished and unseen = ring;
   ended = dimmed (40 %) and struck through. Reuse `paint_agent_mark` (`ui/projects.rs`) and the same pulse as
   `ui/tabs.rs` — do not write other visual code for the same state.
3. **An agent is identified by its model's logo** (`Icon::ClaudeLogo`, `AntigravityLogo`, `CodexLogo`), in the agent's
   color; the dot (`dot`) only for what has no logo (terminal).
4. **Hierarchy by indentation and guide lines**, like the file explorer (`agent_tree`/`paint_guides`).
5. **No gratuitous decoration.** Accent color only for state or selection; never paint a whole header in an agent's
   color (several agents share the same project).

Decisions the user already made (do not reopen unless they ask):
- "+ New agent" lives **inside** the active project, at the end of its agent list; the "+" of the PROJECTS header
  creates a project. No loose "+" in the title bar.
- The ORGANIZATION section stays a **section** (today `Tabs | Split | Highlight`), not an icon in the footer. The window
  of the active tab has a 1 px border and a 3 px halo in the agent's color, and the others sit at 62% (`theme::PANE_*`).
  Footer with shortcuts: not for now.

## Text and language

- **Every text the user reads goes through `t!`** (`use holt_core::t;`): `t!("Close")` is `&'static str`,
  `t!("Model: {model}", model = m)` is `String`. The English text is the key; add the pair `("English", "Português")`
  to the area file in `crates/core/src/i18n/pt_br/` (see `memory/learnings/i18n-english-key-and-area-files.md`).
  The app is English by default; pt-BR is chosen in ☰ > Language and switches at once.
- Write the English first, short sentences; tooltips say what the click does. The pt-BR entry keeps the same meaning
  and the same `{placeholders}`. `cargo test -p holt-core i18n` fails on a `t!` without a pt-BR entry.
- Measure widths from the translated text (`text_w(&painter, t!("..."), ...)`), never from a fixed width that only
  fits one language: Portuguese is usually 20–30% longer. Check both languages in the screenshot when a label is tight.
- Text only agents read (instructions, briefs, ask-agent output) never goes through `t!`: it is English only.
- Code comments are in English.

## Tokens (`crates/app/src/theme.rs`)

Colors come from the semantic palette: `theme::pal().<token>` (`Palette`, `DARK`). Never write a loose color: the test
`colors_come_from_the_palette` fails on `Color32::from_rgb`/`from_rgba_*`/`from_gray`/`theme::rgba(` outside
`theme.rs`, `terminal/colors.rs` and `icons.rs` (the `STILL_LOOSE` list, empty today: it only shrinks).

| Token | Value | Use |
| :-- | :-- | :-- |
| `bg` | #16181c | window background, behind the islands |
| `glass` / `glass_border` / `glass_highlight` | white-gray 50% / white 13% / white 8% | background, 1 px border and top highlight of the islands |
| `well` | rgba(22,24,28,.92) | central island (terminal) |
| `strip` | rgba(44,47,54,.55) | tab strip |
| `separator`, `guide` | white 10% / 8% | lines inside islands, tree guides |
| `hover`, `selected`, `selected_soft` | white 7% / 12% / 8% | hover, neutral selection, selected file |
| `recess`, `chip` | black 25% / white 7% | track of controls and fields; chips |
| `popup` | #23262c | menus, modals, tooltips (opaque) |
| `text`, `text_strong`, `text_2`, `text_3`, `text_soft`, `text_faint` | #e6e8eb, #fff, #aab1bb, #939aa4, #c9cdd3, #6b717c | text: main, selected, secondary, labels/metadata, values, separators |
| `on_accent` | #16181c | text on the accent color |
| `danger`, `danger_text`, `success`, `warning`, `info` | | states |
| `scrim`, `shadow` | black 50% / 28% | modal scrim, island shadow |

Agent colors (not in the palette): `CLAUDE`, `ANTIGRAVITY`, `SHELL` or `deck.agents.tab_color(kind)`.
Terminal: `TERM_BG` #16181c, `TERM_FG`, `TERM_SELECTION` #2f3542.

**The accent color is the active agent's**, never a fixed brand color: `deck.accent()` (or `theme::accent()`, which
`ui::draw` updates every frame; with no tab, neutral gray `#c9cdd3`). It marks selection, primary action, active divider
and the glow in the background's corner. Text on it: `on_accent`. Helpers: `accent_soft` (18%, selected row),
`accent_wash` (10%, focused card), `accent_edge` (50%, card border), `with_alpha`, `with_opacity`, `brighten`, `over`.

Fonts (embedded, OFL, `assets/fonts/`): `theme::ui(px)` Inter, `ui_medium`, `ui_semibold`, `ui_bold`; `theme::label(px)`
IBM Plex Mono Medium for UPPERCASE section labels (10 px, 0.1 em spacing, `ui::section_label`); `theme::meta(px)` Plex
Mono Regular for metadata (10–10.5 px). Base text 12 px. The terminal keeps the system font. Symbols (▾ ▸ ✕) fall back
to the system font.

## Islands and background

- Bars and center are **islands** with 10 px corners (`ISLAND_RADIUS`), with a 6 px margin and gap (`ISLAND_GAP`), on
  the `bg` background with two radial glows (`ui/background.rs`, fan `Mesh`). No blur (egui has none).
- Order: `island_shadows` (shadow only in the gaps, the glass is translucent) → `island(ui, rect, kind)` (`Glass`/`Well`)
  → content → `island_edge(ui, rect)` (repaints the corners in the background color, border and highlight). Each window
  of the split view is a `Well` island. Dividers sit in the gap (`paint_splitter` only shows with the mouse).
- Common components in `ui/mod.rs`: `segmented`, `button` (`ButtonKind::{Primary, Secondary, Ghost}`), `button_danger`,
  `section_label`, `chip`, `separator`, `galley_spaced`.
- Logo: `Icon::Holt` (`icons.rs`, `LOGO_CARDS`); the `.ico` comes from `scripts/generate-icon.ps1`.

## Measurements

- Sidebar, top to bottom: the Agents | Documents switch (`docs::switch`, 28 px track with 24 px segments, 6 px margin,
  active segment in the accent color; "Docs" if the segment cannot fit "Documents n"; with the sidebar collapsed it moves
  to the title bar, compact, 24 px), `separator`, PROJECTS, ORGANIZATION (`segmented` without accent, 24 px track, 10 px
  margin), FILES. Separators as wide as the island.
- Sidebar rows have a 4 px margin (the selection does not touch the edge): project 22 px (▾/▸, name in 600, count in
  `text_3`, 6 px ring of the agent that finished unseen), **agent 24 px** (`AGENT_ROW_H`), "+ New agent" 22 px in
  `text_3`, file 20 px (11.5 px; `FILE_ROW_H`).
- Agent tree: model icon (12 px in a 16 px box) centered at x 30 of the row, 12 px indent per level (up to 5), name in
  Inter 12 px/500 (600 if selected) 7 px after the box, **model on the right** in `meta(10)`, `text_3`, shortened by
  `projects::model_tag` (`sonnet`, `argon`, `flash`): the name is what gets cut (ellipsis), nothing wraps. Selected:
  `accent_soft` + 2 px stripe in the accent color, name `text_strong`, model `text`.
- Explorer: 12 px indent, extension icons in `text_3` (gray, less color), Git letter (M, A, ?, D, R, U) in `meta(10)`
  `text_2`; selected `selected_soft`, hover `hover`.
- Tabs (`ui/tabs.rs`): 30 px strip (`HEAD_H`), 22 px pills, corners 5, 3 px between them, padding 8; active in the
  agent's color, text `on_accent` 600 and a light shadow; without window or document focus, the active one is `selected`
  + `text_strong`; 7 px dot (halo working, 1.5 px ring finished unseen, 40% with no process); the ✕ is drawn (Inter does
  not have it). `StripKind::colored` picks between agent and document.
- Icon buttons: in the title bar 26×24, corners 5; in headers 20×20, corners 4; hover `hover`.
- Row of a diff's file list: 20 px, selected `selected_soft`; letters and `+`/`−` in `meta(10)`.
- Right bar: 32 px header (`TOPBAR_H`) with 3 named pill tabs (Tasks, Git, GitHub; 22 px tall, corners 5, 10 px pill
  badge). There is no Chat tab: agent messages live in each task's Timeline and system notices are notifications.
- Tasks panel (`ui/tasks.rs`, pure rules in `tasks_ui.rs`) is a **queue grouped by who moves next** (`Lane`: NEEDS YOU,
  AGENTS WORKING, DRAFTS, FINISHED, each a `section_label` with its count in `meta(10)` `text_faint`). A row (8 px inset,
  corners 7): the **state glyph** in an 18 px column (`paint_mark`, one per `Mark`: filled ring review, ring + check test it,
  amber "!" merge stopped, red "!" failed, pulsing dot in the agent's colour working, amber pulse resolving a conflict,
  dashed ring queued, hollow ring draft, green check completed, faint struck ring discarded), the title (12.5 px 600, two
  lines) with the wait on the right (`age_short`, 10.5 px `text_3`), then the state word (11 px 600 in `mark_color`: accent
  for what needs you, `warning`, `danger_text`, `text_2`), the agent's 12 px logo + model tag · effort, the id (`meta`) and
  `+N −M` right-aligned (`paint_diff_counts`, `success`/`danger_text`). NEEDS YOU rows have a 3.5 % white fill and their
  24 px actions (primary, secondary, ghost); only the first one gets `accent_wash` + `accent_edge`. FINISHED rows are 24 px.
  The fixed `+ New task` footer stays.
- Task detail: header (← · id in `meta` · title · ⋯ menu), title 15 px 600, the state sentence with its glyph, agent and
  branch lines, the **lifecycle track** (five 12 px stops on a 2 px line: done `text_2`, current accent/amber/red with a
  9 px halo, label 10.5 px and time `meta(10)` under each), callouts (`notice_box`), the **pane switch** (26 px `recess`
  track, segments sized to their text + count, selected `selected`), the pane, and the **actions pinned at the bottom**
  (Return/Reject open their field there). Changes rows: path cut from the start, a 44×4 green/red bar, the count.
  Timeline: a 1 px guide (reserved with `painter.add(Shape::Noop)` and filled at the end so it sits behind the dots),
  hollow dots for events, filled dots in the speaker's colour for messages, messages in a 7 px-corner bubble.
- Notifications (`ui/notices.rs`, state in `notices.rs`): the **bell** (26×24, `Icon::Bell`) left of the window buttons
  with a 13 px count pill (red with an unread error, amber with only warnings); its **history popover** (340 px, `popup`,
  corners 10) and the **toasts** (320 px, corners 9, `popup`, red border for errors, a 1.5 px line for the time left)
  stacked from the right bar's bottom-right corner, above its footer (`TOAST_BOTTOM`). The tone marks (`tone_mark`, 14 px):
  outlined "i" info, green check success, amber triangle warning, red cross error, accent ring + dot for a task notice.
- Text fields: `recess` background, corners 6, `separator` border (or `accent` when focused).
- Menus, tooltips and modals: `frame(p, r, 6–10, popup, glass_border)` + `shadow(...)`; 26 px menu items with `hover`;
  modal titles `13 px 600`; menu position with `menus::keep_on_screen` (do not use `Area::constrain`, it closes the menu
  on the click — see the comment in `menus.rs`).
- Title bar: `TITLEBAR_H = 32`, no background; window buttons 40×32. Sidebar: 150–480 px (default 220).
  Right bar: 280–640 (default 330).

## Code patterns

- **Helpers in `ui/mod.rs`:** `galley`, `galley_trunc` (ellipsis), `place`, `text_in`, `text_w`, `dot`, `frame`,
  `shadow`, `clickable` (hand cursor), `tip` (hover tooltip; every action without text needs one), `paint_splitter`.
- **Interaction:** paint first and decide later. Keep the action in a variable inside the closure (`switch_to_id`,
  `agent_click`...) and apply it to `deck` **outside** it (the `Deck` borrow does not allow it before).
- **Stable ids** for what can be dragged or has state across frames: `Id::new(("agent-row", &proj.id, &ag.id))`, not
  the automatic id from `allocate_exact_size`.
- **Ephemeral state** (menu open, drag, anchors) in `ctx.data_mut(|d| d.insert_temp(...))`; clear it when the section
  goes away (e.g. collapsed bar). **State the user expects back** goes to the config (`Config`/`ProjectCfg` in the core),
  not to `ctx.data`.
- **Overlay above everything** (drag ghost, tooltips): `ctx.layer_painter(LayerId::new(Order::Tooltip, ...))`.
- **Animation:** `ctx.animate_bool_with_time(id, on, 0.12–0.15)`; pulse with `ctx.input(|i| i.time)` and
  `request_repaint_after(33 ms)` only while the state lasts.
- **`dragging`** (parameter of the sidebar sections) turns hover off while something is being dragged.
- **Logic out of the painting:** layout/tree/order computation goes to a pure function with tests in the same file
  (e.g. `agent_tree`, `in_subtree`, `split.rs`). The real state (instances, team) lives in the core.

## Check the result (screenshot)

Build and run in a separate data folder (the user's is in use), with the fake projects:

```sh
cargo build -p holt
HOLT_HOME=<scratch>/home HOLT_SMOKE=<scratch>/shot HOLT_DEBUG_UI=projects-demo \
  ./target/debug/holt.exe        # saves <scratch>/shot/screen.png and exits
```

For Portuguese, write `{"language": "pt-BR"}` to `<scratch>/home/cfg/config.json` before running. Other states in
`HOLT_DEBUG_UI` (list in `debug_ui::apply`, `crates/app/src/debug_ui.rs`): `settings`, `newtab`, `usage-demo`,
`git-demo`, `split-drag`, `tasks-demo`, `tasks-detail`, `tasks-conflict`, `tasks-stopped`, `tasks-messages`,
`tasks-validating`, `tasks-new`, `notices`, `notices-open`, `branch:<name>`... For the README screenshot
(`docs/screenshots/main.png`, 1920×998) a scratch `HOLT_HOME` whose config opens this repository maximized with two
Claude tabs, plus `projects-demo,tasks-demo,usage-demo,branch:dev` and `HOLT_SMOKE_DELAY=12000` (Claude needs the time to
draw); the app rewrites the managed block of the project's `AGENTS.md`, so `git checkout AGENTS.md` afterwards. Open the image and check alignment, truncation and contrast before saying you are done;
interaction (click, drag) does not show in the screenshot — tell the user what is left to test by hand.
