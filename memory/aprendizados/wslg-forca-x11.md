---
title: Under WSLg the window must open using X11, not Wayland
summary: WSLg's Weston 9 breaks Wayland clients (and can crash); the app forces X11 when `/mnt/wslg` exists.
touches: [crates/app/src/main.rs, crates/app/src/wslg.rs]
---
Under WSLg the desktop runs **Weston 9** (Wayland) alongside XWayland. Weston 9's Wayland client implementation breaks
with windows that draw client-side decorations and handle resizing — incorrect window borders, missing cursor, and
Weston itself has crashed (`rust-windowing/winit#4306`, `microsoft/wslg#1008` and `#633`). XWayland is reported as the
stable route; the app now launches through it when WSLg is detected.

**Why:** `winit` defaults to Wayland whenever `WAYLAND_DISPLAY` exists (which is the case under WSLg), and forcing the
backend via `EventLoopBuilderExtX11::with_x11()` takes precedence over that choice and over `WINIT_UNIX_BACKEND`.

**How to apply:** detection (`crates/app/src/wslg.rs`) requires WSL (`WSL_DISTRO_NAME` or "microsoft" in
`/proc/sys/kernel/osrelease`) **and** the directory `/mnt/wslg` (which only exists when GUI support is active); the pure function
has unit tests. An explicit `WINIT_UNIX_BACKEND` still takes precedence for anyone wanting to experiment with Wayland there.
Two WSLg limitations that the app cannot resolve: under X11 scaling comes from XSettings (`Xft/DPI`, which WSLg does not
publish), so it is always 1 and the UI renders small on high-DPI displays — and `winit` 0.30 **no longer includes**
`WINIT_X11_SCALE_FACTOR` (it was removed; do not document this workaround as if it worked). Undecorated maximized windows
have also appeared offset by 32 px. For Windows, running the native application is the recommended path.
