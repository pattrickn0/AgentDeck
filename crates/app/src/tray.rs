//! System tray icon: the window's close button (×) hides the app here (like Discord) instead of quitting,
//! and the icon brings the window back or quits for good. Windows: `tray-icon` (Win32). Linux: `ksni`
//! (StatusNotifierItem via D-Bus, no GTK). Without a system tray (no `StatusNotifierWatcher` on D-Bus,
//! or other system), the close button minimizes: the app never becomes invisible with no icon.

use std::sync::Arc;

use holt_core::t;

/// What the tray menu (or a click on the icon) requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// Shows the window (restoring if minimized) and brings it to the foreground.
    Open,
    /// Quits the app, through the same path shutdown has always used (`Deck::request_quit`).
    Quit,
}

/// Called from any thread when the user interacts with the icon.
pub type OnCommand = Arc<dyn Fn(TrayCommand) + Send + Sync>;

/// What the window's close button (×) does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Hides the window; the tray icon brings it back (and its "Quit" closes the app).
    Hide,
    /// Quits, as the tray's "Quit" does: without a tray icon there is nothing to come back from and nowhere else to
    /// quit (common on Linux: GNOME without the AppIndicator extension, WSLg).
    Quit,
}

pub fn close_action(tray_up: bool) -> CloseAction {
    if tray_up {
        CloseAction::Hide
    } else {
        CloseAction::Quit
    }
}

/// The tray icon; disappears when dropped.
pub struct Tray {
    icon: imp::Icon,
    /// The counts the tooltip shows now (`set_waiting` only touches the system when they change).
    waiting: (usize, usize),
}

/// The tooltip's lines: the name, then what waits for the user (tasks in review, tasks awaiting validation).
/// Empty counts give just the name.
pub fn tooltip_lines(review: usize, validating: usize) -> Vec<String> {
    let mut lines = vec!["Holt".to_owned()];
    match review {
        0 => {}
        1 => lines.push(t!("1 task waiting for your review").to_owned()),
        n => lines.push(t!("{count} tasks waiting for your review", count = n)),
    }
    match validating {
        0 => {}
        1 => lines.push(t!("1 task awaiting your validation").to_owned()),
        n => lines.push(t!("{count} tasks awaiting your validation", count = n)),
    }
    lines
}

impl Tray {
    /// Creates the icon with brand `rgba` (`size`×`size`). `None`: tray is unavailable (the close button quits instead).
    /// Call from the event loop thread (on Windows the icon lives on it).
    pub fn start(rgba: Vec<u8>, size: u32, on: OnCommand) -> Option<Tray> {
        match imp::start(rgba, size, on) {
            Ok(icon) => Some(Tray { icon, waiting: (0, 0) }),
            Err(e) => {
                log::warn!("system tray unavailable ({e}): window close button will minimize");
                None
            }
        }
    }
}

impl Tray {
    /// Shows in the icon's tooltip what waits for the user's decision (tasks in review and awaiting validation), so a
    /// delivered task is not forgotten while the window is hidden. Cheap to call every frame.
    pub fn set_waiting(&mut self, review: usize, validating: usize) {
        if self.waiting == (review, validating) {
            return;
        }
        self.waiting = (review, validating);
        imp::set_tooltip(&self.icon, tooltip_lines(review, validating));
    }
}

/// RGBA (as winit and `tray-icon` expect) → ARGB32 in network order (`IconPixmap` for StatusNotifierItem).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rgba_to_argb(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>().0.iter().flat_map(|[r, g, b, a]| [*a, *r, *g, *b]).collect()
}

#[cfg(windows)]
mod imp {
    use holt_core::t;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::{OnCommand, TrayCommand};

    pub type Icon = TrayIcon;

    pub fn set_tooltip(icon: &Icon, lines: Vec<String>) {
        if let Err(e) = icon.set_tooltip(Some(lines.join("\n"))) {
            log::warn!("could not update the tray tooltip: {e}");
        }
    }

    pub fn start(rgba: Vec<u8>, size: u32, on: OnCommand) -> Result<Icon, String> {
        let (open, quit) = (MenuItem::with_id("open", t!("Open"), true, None), MenuItem::with_id("quit", t!("Quit"), true, None));
        let menu = Menu::new();
        menu.append_items(&[&open, &quit]).map_err(|e| e.to_string())?;
        let icon = tray_icon::Icon::from_rgba(rgba, size, size).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Holt")
            .with_menu(Box::new(menu))
            // Left click opens the window; right click shows the menu.
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;
        let click = on.clone();
        TrayIconEvent::set_event_handler(Some(move |ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                click(TrayCommand::Open);
            }
        }));
        MenuEvent::set_event_handler(Some(move |ev: MenuEvent| match ev.id.as_ref() {
            "open" => on(TrayCommand::Open),
            "quit" => on(TrayCommand::Quit),
            _ => {}
        }));
        Ok(tray)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use holt_core::t;
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::StandardItem;
    use ksni::MenuItem;

    use super::{rgba_to_argb, OnCommand, TrayCommand};

    pub struct Sni {
        argb: Vec<u8>,
        size: i32,
        on: OnCommand,
        /// The tooltip's lines: the title, then the description.
        lines: Vec<String>,
    }

    impl ksni::Tray for Sni {
        fn id(&self) -> String {
            "holt".into()
        }

        fn title(&self) -> String {
            "Holt".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            vec![ksni::Icon { width: self.size, height: self.size, data: self.argb.clone() }]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: self.lines.first().cloned().unwrap_or_default(),
                description: self.lines.iter().skip(1).cloned().collect::<Vec<_>>().join("\n"),
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.on)(TrayCommand::Open);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let item = |label: &str, cmd| {
                StandardItem { label: label.into(), activate: Box::new(move |t: &mut Sni| (t.on)(cmd)), ..Default::default() }.into()
            };
            vec![item(t!("Open"), TrayCommand::Open), item(t!("Quit"), TrayCommand::Quit)]
        }
    }

    pub type Icon = Handle<Sni>;

    pub fn set_tooltip(icon: &Icon, lines: Vec<String>) {
        // `None` = the tray service is gone; there is nothing to update.
        let _ = icon.update(|tray| tray.lines = lines);
    }

    pub fn start(rgba: Vec<u8>, size: u32, on: OnCommand) -> Result<Icon, String> {
        // Without `StatusNotifierWatcher` on D-Bus, `spawn` returns an error (we do not use `assume_sni_available`).
        Sni { argb: rgba_to_argb(&rgba), size: size as i32, on, lines: vec!["Holt".to_owned()] }.spawn().map_err(|e| e.to_string())
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::OnCommand;

    pub struct Icon;

    pub fn set_tooltip(_icon: &Icon, _lines: Vec<String>) {}

    pub fn start(_rgba: Vec<u8>, _size: u32, _on: OnCommand) -> Result<Icon, String> {
        Err("no system tray on this system".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tooltip_says_what_waits_for_the_user() {
        use holt_core::i18n::english;
        english(|| {
            assert_eq!(tooltip_lines(0, 0), ["Holt"]);
            assert_eq!(tooltip_lines(1, 0), ["Holt", "1 task waiting for your review"]);
            assert_eq!(tooltip_lines(3, 2), ["Holt", "3 tasks waiting for your review", "2 tasks awaiting your validation"]);
        });
    }

    #[test]
    fn closing_hides_only_when_there_is_a_tray_to_come_back_from() {
        assert_eq!(close_action(true), CloseAction::Hide);
        assert_eq!(close_action(false), CloseAction::Quit, "without a tray the × quits: nothing else could close the app");
    }

    #[test]
    fn the_status_notifier_wants_argb_in_network_order() {
        assert_eq!(rgba_to_argb(&[1, 2, 3, 4, 10, 20, 30, 255]), vec![4, 1, 2, 3, 255, 10, 20, 30]);
    }
}
