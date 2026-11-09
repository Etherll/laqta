# ADR 028: One design system for the views: tokens, shared components, bundled fonts and icons

Status: accepted (2026-11-08).

**Context.** ARCHITECTURE §5 asks for one theme token set applied through `gpui-component` theming.
Before this, each window chose its own sizes, radii and colours inline (`px(6.)` corners, raw
`rgb(0xef4444)`), the windows used the platform UI font, and no icon asset source was registered,
so `gpui-component` icons did not draw.

**Decision.**
- `laqta-ui/src/theme.rs` holds the token set: a neutral grey palette for light and dark with a
  green `primary` kept for the one key action in a region, a fixed radius scale (8–28px, pills for
  anything one line tall), a 4px spacing rhythm and a type scale (caption 11.25px up to page
  title 31.9px). `apply` lays the palette over `gpui-component`'s theme, so its buttons, inputs
  and switches follow it. The colours the components need beyond that theme sit in one `Extra`
  set: the card `surface`, `info` (busy state, pin border, selection), `warning` (paused
  recording) and `close_hover`. There are no motion tokens.
- `laqta-ui/src/widgets.rs` holds the shared components built on those tokens: card, section,
  settings row, page title, pill button (default, primary, dark, ghost), nav row, segmented
  control, badge, key hint and empty state. Interactive ones are tab stops with a focus ring.
  Views compose these instead of styling inline.
- Inter (body) and Space Grotesk (headings, requested as "Space Grotesk Light" because that is the family name the bundled file reports), both SIL OFL 1.1, are embedded in `laqta-ui` with their
  licence files and registered with GPUI the first time a window is about to open, so the tray-only
  process never loads them (§1.4, risk R6).
- Icons come from `gpui-kit-assets` (Apache-2.0, part of GPUI Kit). `laqta-ui` registers one asset
  source: the Lucide icons the views name plus the kit's default component icons. The crate is
  pinned `=0.9.0` and moves with `gpui-component` (ADR 002).
- Windows draw their own titlebar (`chrome.rs`): client-side decorations, a 36px bar in the
  sidebar colour with the title and 46px window controls, drag to move, double click to maximize,
  right click for the window menu. The content sits on a sheet with a rounded corner where it
  meets the sidebar. Where the system decorates the window anyway (X11 without a compositor, a
  compositor that refuses client-side decorations), the bar draws nothing, so the system frame is
  never doubled. Windows without a sidebar use a flat bar in the page colour.
- Floating windows have 10px rounded corners on Linux (square on edges tiled against the screen).
  `gpui-component` 0.9.0 keeps its client-side frame square because GPUI's content masks are
  rectangular, so it is vendored in `vendor/gpui-component` with a small patch: the frame and the
  root surface are rounded, and `window_corner_radii` lets views round the backgrounds they paint
  into a corner (`chrome::rounded`, the titlebar, the content sheet). The patch is re-applied when
  GPUI Kit moves to a new snapshot (ADR 002); `vendor/README.md` lists it.
- View logic that decides what to show (a status's tone, a hotkey's label) lives in presenters, as
  for every view (ADR 017).

**Rejected.**
- Inline styling per window: the windows drift apart, and dark mode has to be fixed in each one.
- A separate design-system crate: one user (`laqta-ui`) and GPUI types throughout, so it would add
  an edge and a crate for nothing.
- The platform UI font: it differs per desktop and changes the layout's metrics with it.
- Square window corners: they look dated next to the rounded content and the reference style.
- The system titlebar: it ignores the theme, and its height and look differ per desktop.
- The full Lucide catalog as assets: about 1,600 SVGs embedded for some 60 used.

**Consequences.** New views build from `widgets.rs` and read colours from the theme. The editor
and photo editor windows keep their own toolbars for now, under the same titlebar (`chrome.rs`;
the photo editor's is flat); they pick up the palette and fonts through `gpui-component` and the
theme font. The radius and type scales are adopted window by window, so a few literal sizes and
radii remain.
