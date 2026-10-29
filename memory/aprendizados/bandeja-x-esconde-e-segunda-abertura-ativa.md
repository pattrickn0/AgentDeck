---
title: The close button hides to system tray and a second launch activates the running instance
summary: Quitting is done via the tray menu; the second instance reads the bridge in dados/.agentdeck.bridge and sends POST /activate with the master token.
touches: [crates/app/src/app.rs, crates/app/src/tray.rs, crates/core/src/activation.rs, crates/core/src/bridge.rs]
---
The window's × (`CloseRequested`, Alt+F4, and the drawn × on the titlebar) **does not quit**: it hides to the system tray
(`App::close_window`). Quitting is done via "Quit" in the tray icon menu (`Deck::request_quit`). Any new code that
touches "closing" must decide which of the two it wants; `Deck`'s `quit_requested` now works while the window is hidden
(`App::quit_if_requested` also runs after `Deck::background`).

**Why:** the user wants Discord-like behavior, with long turns finishing without the window being open. A hidden window
receives no frames, so three things work only because they were handled manually: `Background::wake` when hiding (winit gives
no notification), `show_window` upon returning (a `request_redraw` and `viewport_stale`), and the "Quit" confirmation prompt only appears after
showing the window. Without a system tray (Linux without `StatusNotifierWatcher`), the × **minimizes**: hiding would leave the app invisible and
with no way to bring it back (`tray::close_action`).

A second launch no longer fails with "another window is already using the folder": `main` calls `activation::activate_running`,
which only acts if the lock `dados/.agentdeck.lock` is held by another process, reads `dados/.agentdeck.bridge` (port and master
token, written after the bridge is up), and sends `POST /activate`. The file alone proves nothing: whoever acquires the lock
deletes the file left by a crash, and an instance still starting up (lock held, file absent) is awaited for up to 6 s.
**How to apply:** `/activate` only accepts the master token (an agent cannot bring the user's window to the front); on
Windows the second instance needs `AllowSetForegroundWindow(ASFW_ANY)` before the POST, otherwise the window merely flashes. To
test the tray without clicking: `PostMessage(hwnd, WM_CLOSE)` hides the window and opening the `.exe` again brings it back.
