# ADR 019: Wayland backends and fallbacks

Status: accepted (2026-11-07)

**Context.** Wayland compositors differ in what they offer:
- GNOME (Mutter) has no `wlr-layer-shell`.
- `xdg-desktop-portal-wlr` has no GlobalShortcuts portal.
- The Screenshot portal has no restore token; only ScreenCast does.

The overlay, hotkeys and the 120 ms budget all depend on these features.

**Decision.**

| Need | Order of backends |
| - | - |
| Still capture | `ext-image-copy-capture-v1`, then `wlr-screencopy-v1`, then the Screenshot portal in non-interactive mode. GNOME asks for permission once and stores it in its permission store. No token is involved |
| Recording | ScreenCast portal with `persist_mode = 2`. `main` reads the restore token from the file `<data>/screencast-token`, passes it to the Linux adapter when building it (`ports_with_token`), and writes new tokens that the adapter reports through a callback (mode 0600). The platform crate never touches storage (ADR 024) |
| Overlay | `WindowKind::LayerShell` on the Overlay layer, anchored to all edges, with exclusive keyboard focus, where `wlr-layer-shell` exists (wlroots compositors, KWin, Hyprland, niri, COSMIC). When the compositor refuses the layer surface (GNOME), `laqta-ui`'s overlay opens a pop-up window sized to each monitor instead |
| Hotkeys | GlobalShortcuts portal (KDE, GNOME 48+, Hyprland). Elsewhere, the `laqta --action <name>` command, forwarded to the running instance over IPC and bound by the user in the compositor's settings. The hotkey settings page is to show the command for each action; it does not yet (it shows "Runs: <label>", and only the tray tooltip names a command) |
| Hover targets | One compositor's IPC in M1 (chosen in the M1 plan); others on request. GPL-3.0 IPC crates (`hyprland`, `niri-ipc`) are licence-compatible since ADR 022. Elsewhere `HoverSource` returns `Unsupported`, and the overlay offers only free-form selection |

Window kinds on the other platforms:
- **Windows:** a borderless, topmost pop-up window per monitor.
- **X11:** an override-redirect fullscreen window per monitor.

**Rejected.**
- **Requiring layer-shell and GlobalShortcuts:** it would exclude GNOME, the most common Wayland desktop.
- **Always using the portal's interactive picker:** it adds a dialog to every capture.

**Consequences.**
- The 120 ms overlay budget applies to Windows, X11 and layer-shell compositors.
- On GNOME, latency is measured but not budgeted.
- Risks R1 and R2 in ARCHITECTURE.md track this.
