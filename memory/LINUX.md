# Linux Support (Ubuntu and Fedora)

Verification performed in tasks `t29` (2026-10-22) and `t33` (2026-10-23). This document explains **what was checked, how**,
what was fixed, the dependencies for each distro, and what remains unverified. The manual Windows test script
remains in [manual-tests.md](manual-tests.md); the Linux script is at the end here.

> **Summary:** the app compiles and passes tests on Ubuntu (CI) and now also on Fedora (container job). The interface
> was manually validated only on Ubuntu X11; Wayland and a real Fedora desktop have not yet been tested. There is no
> package, installer, or `.desktop` file. Antigravity quota now reads the system keyring (t33), and under WSLg the
> app opens via X11 (t33) — neither has been exercised on a real Linux machine.
>
> **2026-10-29:** the user moved to Linux; the development machine is now **openSUSE Tumbleweed with GNOME on Wayland**
> (not covered by CI, which runs Ubuntu and Fedora). The first real Wayland run found the bugs in section 2.9.

## 1. How this verification was done

There was no Linux machine, WSL, or Docker on this machine, so verification was:

1. **GitHub CI** (repository `pattrickn0/AgentDeck`), reading Ubuntu and Windows job logs via the
   `/actions/jobs/<id>/logs` endpoint, plus the workflow runs API.
2. **Code review** of everything that has platform-specific paths: `process.rs`, `paths.rs`, `agents.rs`, `git.rs`,
   `github.rs`, `workspace.rs`, `whisper.rs`, `download.rs`, `usage/*`, `theme.rs`, `terminal/*`, `ui/projects.rs`,
   `main.rs`, `crash.rs`, and tests with `#[cfg(unix)]`.
3. **Verification of downloaded artifacts**: names and SHA-256 hashes of whisper.cpp packages in release `b5454` match
   the constants in `whisper.rs` (Ubuntu x64 and arm64).
4. **Dependency review** in `Cargo.lock`: no `gtk-sys`/`glib-sys` (`rfd 0.17` uses
   `xdg-portal` + `wayland`), and `ureq` uses rustls (no OpenSSL).

`cargo check --target x86_64-unknown-linux-gnu` was attempted, but stops at `ring` because there is no
`x86_64-linux-gnu-gcc` toolchain on this machine — which is why the Linux build is verified only via CI.

The Antigravity quota and WSLg (sections 2.7 and 2.8) were verified by code review and upstream sources
(`zalando/go-keyring`, `winit` and WSLg issues). The keyring module compiles for `x86_64-unknown-linux-gnu`
in a separate crate (as in `t29`, the workspace stops at `ring` due to the lack of a cross-gcc), and the code
forcing X11 was verified the same way; but there was no Linux or WSL available to run the app.

## 2. What was broken and got fixed

### 2.1 The test that failed only on Linux (since Oct 17)

Ubuntu jobs (`runs 42–46`) failed in `workspace::tests::read_dir_orders_and_filters`, always with
`index out of bounds: the len is 0 but the index is 0` in `workspace.rs`. The line was:

```rust
let sub_bs = read_dir(ws, "zeta\\");
assert_eq!(sub_bs[0].rel_path, "zeta/in.json");   // panic on Linux
```

`read_dir` validated and joined the path **before** replacing `\` with `/`. On Windows `zeta\` is the directory `zeta`;
on Unix the backslash is part of the filename, so `ws.join("zeta\\")` looks for a file named `zeta\` and returns
empty. Fix in `crates/core/src/workspace.rs`: normalize the separator at the beginning of `read_dir`. The test remains
valid (and passes) on Windows. See
[learnings/backslash-path-in-read-dir.md](learnings/backslash-path-in-read-dir.md).

### 2.2 CI stopped validating Linux (from Oct 22 until formatting debt was paid)

Starting with `run 47` (commit `146a486`), the **Formatting** step (`cargo fmt --all -- --check`) failed on Windows **and**
Ubuntu, and the build, test, and clippy steps didn't even run. The debt was paid later: on 2026-10-24 CI passed completely
across all three jobs (Windows, Ubuntu, Fedora), including formatting.

### 2.3 Data folder in system-wide installation

`AppPaths::detect()` always used the executable's directory for `data/`, `workspace/`, and `whisper/`. Copying the binary
to `/usr/local/bin` (root-owned directory) caused the app to fail when opening the lock (`data/.agentdeck.lock` →
permission denied). Now, on Unix, if the folder is not writable (`access(2)`), data goes to `~/.local/share/AgentDeck`
(XDG standard). Portable folder and development repository continue as before, and `AGENTDECK_HOME` still takes
precedence. Fix in `crates/core/src/paths.rs`, with a test under `#[cfg(unix)]`.

### 2.4 Reference to a non-existent script

The dictation error message instructed compiling whisper.cpp with `scripts/build-whisper-linux.sh`, which does not exist
in the repository. The message now explains what to actually do (compile with Vulkan/CUDA and place `whisper-cli` in
`PATH`, or use the CPU engine).

### 2.5 Terminal fonts on Fedora

Linux font paths only covered the Debian/Ubuntu layout (`/usr/share/fonts/truetype/...`). Fedora installs each
family into its own directory (`/usr/share/fonts/dejavu-sans-mono-fonts/...`), so the list found nothing and the
terminal fell back to egui's built-in font. `theme.rs` gained Fedora paths (the UI already uses embedded Inter/IBM Plex;
this applies to the terminal and symbols).

### 2.6 Documentation and CI

- `README.md` and `CONTRIBUTING.md`: dependencies corrected (no GTK3, which is unused), `xdg-utils`/portal, `libgomp`,
  data folder, new repository URL (`AgentDeck`).
- `.github/workflows/ci.yml`: removed `libgtk-3-dev`; added a **`fedora`** job (`container: fedora:latest`) that
  installs README dependencies and runs `cargo build --workspace` and `cargo test --workspace`.

### 2.7 Antigravity quota via system keyring (t33)

On Linux, `agy` does not store the token in `~/.gemini/oauth_creds.json`: it uses **Secret Service** (the GNOME/KDE
keyring over D-Bus), with `service=gemini`, `username=antigravity` and the same JSON as Windows Credential Manager.
The app previously only checked the file, so the quota remained "no data". `crates/core/src/usage/antigravity.rs`
gained keyring reading via the `secret-service` crate (pure Rust, over `zbus`; no new C libraries in CI), running in
its own thread with a 3-second timeout and only for already unlocked items — unlocking would trigger a password prompt
in the background. Without a D-Bus session (SSH, container), the lookup silently fails and the app falls back to files
as before; keyring reading only occurs when the home used is the system home, as was already the case on Windows. Also
added support for the file `agy` writes when there is no keyring (`GEMINI_FORCE_FILE_STORAGE=true`, the SSH,
container, and WSL fallback): `~/.gemini/antigravity-cli/antigravity-oauth-token`, after `~/.gemini/oauth_creds.json`
in source precedence. See [learnings/antigravity-token-in-secret-service.md](learnings/antigravity-token-in-secret-service.md).

To verify: `secret-tool lookup service gemini username antigravity` (`libsecret-tools` package on Ubuntu, `libsecret`
on Fedora) prints the JSON and prompts to unlock the keyring if locked. With a valid token, quota appears on the next
poll (every 3 min, or 20 s after a failure). If it doesn't appear, run with `RUST_LOG=debug`: `agentdeck.log` records
when the keyring did not return a token.

### 2.8 WSLg: open via X11 (t33)

Under WSLg, the desktop is Weston 9 (Wayland) alongside XWayland. The Weston 9 Wayland client breaks windows that
draw client-side decorations and resize, and Weston has crashed with Wayland clients (`winit#4306`,
`microsoft/wslg#1008` and `#633`). The app now detects WSLg (`WSL_DISTRO_NAME` or "microsoft" in
`/proc/sys/kernel/osrelease`, plus the `/mnt/wslg` directory) and forces `winit`'s X11 backend
(`crates/app/src/wslg.rs`, with tests for the pure function; `main.rs`). Outside WSLg nothing changes, and a
user-defined `WINIT_UNIX_BACKEND` still takes precedence.

What the app does not resolve, and why the README advises using the native app on Windows:

- **Scale always 1**: on X11 scaling comes from XSettings (`Xft/DPI`), which WSLg does not publish; on high-density
  displays the UI appears small. `winit` 0.30 no longer has `WINIT_X11_SCALE_FACTOR` (it was removed), so there is
  no environment variable workaround — fixing this requires handling scaling in the app.
- **Maximized window without decorations**: has appeared shifted by 32 px in WSLg; without a Windows machine with WSLg,
  this cannot be confirmed or fixed.
- **Resizing and cursor**: these are precisely the failure points on WSLg Wayland; on X11 they should work, but nobody
  has opened the app there.

See [learnings/wslg-forces-x11.md](learnings/wslg-forces-x11.md).

### 2.9 First real run: openSUSE Tumbleweed, GNOME on Wayland (2026-10-29)

- **× did nothing**: closing to the tray used `set_visible(false)`, a no-op on Wayland. Now the window is destroyed and
  its GL context parked (`GlWindow::park`); the tray's "Open" makes a new window on the same context
  (`ParkedGl::reopen`), so egui's textures survive. Checked with a scripted close/reopen; X11 and Windows still hide.
- **Crash on every exit** (`SIGSEGV` in `wl_proxy_destroy`): `run_app` closes winit's Wayland connection before `App`
  is dropped, and egui-winit's clipboard and the window were dropped after it. `App::exiting` now drops them first.
- **Generic icon, no app-menu entry**: Wayland ignores the window icon. The window now has app id `agentdeck`
  (`WM_CLASS` on X11), and `cargo build --release` installs `agentdeck.desktop` (from `crates/app/assets/`, `Exec` =
  the built binary) and the icon under `~/.local/share` (`crates/app/build.rs`; not in CI, not when cross-compiling,
  not with `AGENTDECK_NO_DESKTOP_ENTRY`). `scripts/install-desktop.sh` does it for another binary, or `--uninstall`s.
- **No terminal font**: openSUSE puts fonts straight in `/usr/share/fonts/truetype/`; those paths were added.

See [learnings/wayland-close-destroys-the-window.md](learnings/wayland-close-destroys-the-window.md).

## 3. Dependencies

### 3.1 To compile

| | Ubuntu 24.04+ | Fedora 41+ | openSUSE Tumbleweed |
| :-- | :-- | :-- | :-- |
| Base | `build-essential pkg-config` | `gcc gcc-c++ make pkgconf-pkg-config` | `gcc gcc-c++ make pkgconf-pkg-config` |
| Audio (default) | `libasound2-dev` | `alsa-lib-devel` | `alsa-devel` |
| Window/keyboard | `libxkbcommon-dev` | `libxkbcommon-devel` | `libxkbcommon-devel` |
| Rust | 1.89+ (`rustup`) | 1.89+ (`rustup`) | 1.89+ (`rustup`) |

Without audio: `cargo build -p agentdeck --no-default-features` avoids ALSA (leaves audio and microphone disabled).

Not needed: GTK3/GTK4 (`rfd` uses the portal), OpenSSL (`ureq` with rustls), X11/Wayland development headers (winit
loads libraries at runtime). `libxkbcommon` is not required at link time, but it is what CI installs and what the
keyboard uses on Wayland.

### 3.2 To run

| Feature | Ubuntu | Fedora | Without this |
| :-- | :-- | :-- | :-- |
| Window | X11 or Wayland + GL driver (`mesa`) | same (`mesa-libGL`) | does not open |
| Open folder / links | `xdg-utils` (`xdg-open`) | `xdg-utils` | silent failure |
| File dialog | `xdg-desktop-portal` + `xdg-desktop-portal-gtk` | same | dialog does not open |
| Terminal font | `fonts-dejavu-core` (or Liberation/Noto) | `dejavu-sans-mono-fonts` | falls back to egui's built-in font |
| Keyring (Antigravity quota) | Secret Service (`gnome-keyring`/`kwallet`) | same | Antigravity quota shows no data |
| Git / GitHub | `git` (and optional `gh`) | `git` (optional `gh`) | Git/GitHub tabs non-functional |
| Dictation (CPU) | `libgomp`, `libstdc++` | `libgomp`, `libstdc++` | downloaded `whisper-cli` will not run |
| Detect GPU | `pciutils` (`lspci`), `nvidia-smi` | same | falls back to CPU engine |

Agent CLIs: `npm install -g` of the same packages as Windows; Antigravity has its own installer. The app looks in
`PATH`, `~/.local/bin`, `~/.claude/local/claude`, and `~/.antigravity/bin/agy`.

## 4. What is covered by code and tests

| Area | Status on Linux |
| :-- | :-- |
| Processes | own process group per child (`process_group(0)`) and `kill(-pid)` on tree; `cfg(unix)` tests in `process.rs` |
| PTY/terminal | `portable-pty` + `alacritty_terminal`; `TERM=xterm-256color`, `COLORTERM=truecolor` |
| Terminal tab | `$SHELL` or `/bin/bash`; no `.cmd` shims (Windows only) |
| Tab environment | `PATH` with app binary directory prepended; `AGENTDECK_*` variables; no `PYTHONUTF8`/MSYS |
| Single instance | file lock at `data/.agentdeck.lock` |
| Git | `PATH` `git`, without shell, `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0`, SSH in `BatchMode` |
| GitHub | token via `git credential fill` and, if available, `gh auth token`; no disk persistence |
| Agents | resolved via `which`/user directories; no `cmd.exe` |
| Documents/workspace | `workspace::read_dir` normalizes `/` and `\`; `read_text` rejects links outside project |
| Whisper (CPU) | downloads `whisper-bin-ubuntu-x64.tar.gz` (or arm64), `chmod 0o755`, `LD_LIBRARY_PATH` with engine directory |
| Usage/quotas | Antigravity reads Secret Service (`service=gemini`, `username=antigravity`) and, without it, `~/.gemini/oauth_creds.json` and `~/.gemini/antigravity-cli/antigravity-oauth-token`; Codex reads `~/.codex/sessions` |
| Interface | embedded fonts; `xdg-open` for folders; `open` (crate) for URLs |

The test suite has 196 tests in app (plus 1 ignored, for real quota), 253 in core (plus 2 ignored), 31 for bridge, and
37 for tasks, in addition to git/github tests; run locally on Windows on 10/23, all pass. On Linux, the only failing
test was `read_dir` (fixed here); the rest ran green on Ubuntu through `run 46`.

## 5. What remains unverified (or unsupported)

1. **Wayland**: runs on GNOME (section 2.9). Still to check there: the title bar after leaving maximized (reported hidden
   under GNOME's top bar; the app draws it correctly, so the suspect is window placement), dragging splitters, paste,
   IME, and file dialog. Workaround: `WINIT_UNIX_BACKEND=x11`.
2. **Real Fedora desktop**: the new job proves build and tests, not window rendering. Still needed: opening the app on a Fedora
   Workstation (GNOME/Wayland) and repeating the manual script at the end of this document.
3. **Packaging**: no `.deb`, `.rpm`, Flatpak or AppImage. The app runs from the compiled folder; a
   release build adds it to the app menu (section 2.9).
4. **Dictation with GPU**: the app does not download CUDA/Vulkan engines for Linux (only CPU). Users with a GPU need to compile
   whisper.cpp and keep `whisper-cli` in `PATH`; detection uses `lspci`/`nvidia-smi`.
5. **File dialog without portal**: in minimal environments (i3, container, SSH) without `xdg-desktop-portal`, the dialog does
   not open. There is no UI warning; consider adding a message.
6. **Texts referencing “Git Credential Manager”**: Git/GitHub error messages were written on Windows and mention GCM and
   “push to log in”. On Linux the standard route is `gh auth login` or `credential.helper=store`; the text should mention both.
7. **“Open folder” ignores failure**: `Command::new("xdg-open").spawn()` without error handling; without `xdg-utils` the
   click does nothing and does not warn.
8. **Runner version**: the `ubuntu-latest` label migrates to Ubuntu 26 starting 2026-10-19; the job base may change without
   notice. If a new dependency appears, check the “System dependencies” step log.
9. **Formatting**: if `cargo fmt --all -- --check` fails again, no job runs tests — including Fedora. The entire codebase
   conforms to rustfmt: run `cargo fmt --all` and commit what it changes (see
   [learnings/whole-codebase-in-rustfmt.md](learnings/whole-codebase-in-rustfmt.md)).
10. **WSLg**: X11 was forced (section 2.8), but nobody has opened the app inside WSLg. Still to check: maximized window
    (32 px shift), scaling (always 1, small UI on 4K), drag/resize, cursor, and Antigravity quota read from the keyring
    there. `WINIT_UNIX_BACKEND=wayland` reproduces the broken path, for comparison.

## 6. Manual test script (Ubuntu and Fedora)

Run in a separate data folder to avoid affecting real configuration:
`AGENTDECK_HOME=/tmp/agentdeck-test ./target/release/agentdeck`.

**Installation and launch**

- [ ] `cargo build --workspace --release` completes without error (Ubuntu and Fedora).
- [ ] App opens on X11 and Wayland (GNOME and KDE, if possible), with correct window size and DPI.
- [ ] Terminal in the “Terminal” tab opens `$SHELL`, accepts accents and colors, and resizes without breaking line wraps.
- [ ] Closing the app and reopening preserves projects, tabs, and sizes; opening a second instance in the same folder
      warns that it is already in use; with a different `AGENTDECK_HOME`, opens both.

**Agents and memory**

- [ ] The “+ New agent” menu lists installed CLIs, and each agent's card opens the correct tab.
- [ ] `ask-agent --list`, `--team`, `--models` work in a terminal outside the app.
- [ ] `ask-agent claude "..."` in a tab and the typed response arrive; Integrated Conversation shows the pair.
- [ ] Create a task (`--task new ... --start`), inspect the worktree, submit with `--task done`, and read the report.
- [ ] `ask-agent --learn` displays learnings; project `AGENTS.md` is updated only where it already existed.
- [ ] Antigravity quota appears in the titlebar when `agy` is logged in; `secret-tool lookup service gemini
      username antigravity` returns the JSON and, with `RUST_LOG=debug`, `agentdeck.log` reports no failure reading the keyring.

**Interface**

- [ ] Sidebar, right panel, tabs, menus, and modals render without clipped text or overlap.
- [ ] Drag dividers, switch to ORGANIZATION/Focus, and toggle Documents (Ctrl+Shift+D).
- [ ] Open a folder via button (requires `xdg-open`) and pick a file in the dialog (requires portal).
- [ ] Copy and paste (Ctrl+C/Ctrl+V) between the app and another program; paste an image as an attachment.
- [ ] Git tab (status, diff, history, stash) and GitHub tab (PRs, issues, Actions) in a real repository.

**Dictation**

- [ ] With microphone: hold shortcut, speak, and see text appear in tab; temporary `.wav` disappears from `data/`.
- [ ] Without microphone or without `libgomp`: app explains what is missing instead of crashing.

**WSLg (from Windows, with app built in WSL)**

- [ ] `./target/release/agentdeck` opens via X11 (log shows “WSLg detected”); window doesn't flicker or vanish, and
      terminal accepts keyboard input.
- [ ] Maximize, restore, drag, and resize; cursor appears and text is not clipped.
- [ ] On a high-DPI display, note the scale (currently always 1, so UI is small) and what is missing.
- [ ] `WINIT_UNIX_BACKEND=wayland ./target/release/agentdeck` reproduces the WSLg Wayland failure, for comparison.

**Wrap-up**

- [ ] `data/agentdeck.log` and `data/crash.log` exist and have no repeated errors.
- [ ] On closing the app, no orphaned `whisper-cli` or agent processes remain (`ps -ef | grep -E "whisper|claude"`).

## 7. Maintenance

- Changed system dependencies, font paths, data directory, or GPU support? Update this file and the README in the same
  commit.
- The CI `fedora` job is minimal: it does not open a window. A failure occurring only there is usually a missing build
  dependency in `dnf install`.
- When adding a path-handling test, run the suite both ways: `cargo test --workspace` on Windows **and** on Linux (CI).
  The `read_dir` issue demonstrates that a test can pass for years on only one OS.
