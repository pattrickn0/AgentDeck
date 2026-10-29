---
title: On Linux AgentDeck does not use GTK3, and dialogs depend on the portal
summary: rfd 0.17 uses xdg-portal (no gtk-sys); do not require gtk3-devel — but without xdg-desktop-portal dialogs will not open.
touches: [README.md, CONTRIBUTING.md, .github/workflows/ci.yml, crates/app/Cargo.toml]
---
The Linux dependency list previously cited `libgtk-3-dev` (Ubuntu) and `gtk3-devel` (Fedora), which **are not used**:
`Cargo.lock` does not contain `gtk-sys`/`glib-sys`, and `rfd 0.17` has `default = ["xdg-portal", "wayland"]` (the `gtk3` feature
exists, but is not enabled). `ureq` uses rustls, so there is also no OpenSSL build dependency.

What Linux actually requires beyond the compiler and ALSA: `xdg-utils` (`xdg-open`, to open folders/links) and
`xdg-desktop-portal` with a backend (`xdg-desktop-portal-gtk` or `-kde`) for file dialogs. Without the portal, the file
dialog does not open and the app provides no warning.

**Why:** rfd's documentation mentions GTK3, making it easy to copy into the README; the result is a heavy build
dependency that alters nothing in the compiled binary. On the other hand, the portal is invisible in `Cargo.toml` (being a D-Bus service)
and was overlooked.

**How to apply:** before adding a system library, check `Cargo.lock` (is the crate present?) and enabled features.
When testing file dialogs in a minimal environment, check `systemctl --user status xdg-desktop-portal`. If the portal ever
becomes an issue in headless/D-Bus-less environments, the alternative is enabling rfd's `gtk3` feature and requiring
GTK again — a product decision, not a silent "fix".
