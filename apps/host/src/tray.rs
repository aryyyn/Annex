//! Menu bar UI: status, the connect URL, a QR code, and quit.
//!
//! # The thread rule this file exists to obey
//!
//! Everything here is AppKit underneath, so it must run on the **main thread**,
//! and the `NSApplication` run loop must own that thread. That is why `run`
//! never returns until the user quits, and why the whole pipeline has to be
//! started *before* it is called.
//!
//! Menu clicks arrive on a channel rather than as callbacks, so they are polled
//! from a timer on the main thread. Every action is deliberately something that
//! can be done without touching AppKit further: copying to the clipboard and
//! opening a browser are both subprocesses, which sidesteps a whole category of
//! main-thread bugs.
//!
//! # Quit has to be a clean shutdown
//!
//! Not `exit()`. The `VirtualDisplay` is removed by its `Drop`, so killing the
//! process instead leaves the user with a ghost monitor they cannot clear
//! without logging out.

use crate::icon;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
use objc2_foundation::{MainThreadMarker, NSDate, NSDefaultRunLoopMode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// What the menu needs to render, refreshed on a timer.
pub struct Status {
    pub url: String,
    pub clients: u64,
    pub fps_out: u64,
    pub source: String,
    pub resolution: (u32, u32),
    /// Resolutions offered, as menu labels. Empty when the source is a real
    /// screen, whose mode is not ours to change.
    pub modes: Vec<String>,
    pub current_mode: String,
    /// Whether clients can drive this Mac. Shown because a capability like
    /// this should never be invisible to the person whose machine it is.
    pub input_enabled: bool,
}

pub struct Tray {
    _icon: TrayIcon,
    item_status: MenuItem,
    item_clients: MenuItem,
    item_url: MenuItem,
    item_input: MenuItem,
    id_copy: tray_icon::menu::MenuId,
    id_open: tray_icon::menu::MenuId,
    id_qr: tray_icon::menu::MenuId,
    id_quit: tray_icon::menu::MenuId,
    /// One entry per offered resolution, in the order they were listed, so a
    /// click maps straight back to an index.
    mode_items: Vec<(tray_icon::menu::MenuId, CheckMenuItem)>,
    url: String,
}

impl Tray {
    pub fn new(initial: &Status) -> Result<Self, Box<dyn std::error::Error>> {
        let (rgba, w, h) = icon::tray_rgba();
        let image = tray_icon::Icon::from_rgba(rgba, w, h)?;

        let menu = Menu::new();

        // Disabled items are being used as read-only status lines, which is the
        // conventional way to show state in a macOS menu bar menu.
        let item_status = MenuItem::new(
            format!(
                "Streaming {} at {}x{}",
                initial.source, initial.resolution.0, initial.resolution.1
            ),
            false,
            None,
        );
        let item_clients = MenuItem::new("No clients connected", false, None);
        let item_input = MenuItem::new(input_line(initial.input_enabled), false, None);
        let item_url = MenuItem::new(&initial.url, false, None);

        // Resolution submenu, only when the source is a display we own.
        let mut mode_items = Vec::new();
        if !initial.modes.is_empty() {
            let submenu = Submenu::new("Resolution", true);
            for label in &initial.modes {
                let checked = *label == initial.current_mode;
                let item = CheckMenuItem::new(label, true, checked, None);
                submenu.append(&item)?;
                mode_items.push((item.id().clone(), item));
            }
            menu.append(&PredefinedMenuItem::separator())?;
            menu.append(&submenu)?;
        }

        let item_copy = MenuItem::new("Copy URL", true, None);
        let item_open = MenuItem::new("Open in browser", true, None);
        let item_qr = MenuItem::new("Show QR code", true, None);
        let item_quit = MenuItem::new("Quit Annex", true, None);

        menu.append(&item_status)?;
        menu.append(&item_clients)?;
        menu.append(&item_input)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&item_url)?;
        menu.append(&item_copy)?;
        menu.append(&item_open)?;
        menu.append(&item_qr)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&item_quit)?;

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(image)
            // Tells macOS to tint the icon for light and dark menu bars rather
            // than drawing our black pixels literally.
            .with_icon_as_template(true)
            .with_tooltip("Annex")
            .build()?;

        Ok(Self {
            _icon: icon,
            item_status,
            item_clients,
            item_url,
            item_input,
            id_copy: item_copy.id().clone(),
            id_open: item_open.id().clone(),
            id_qr: item_qr.id().clone(),
            id_quit: item_quit.id().clone(),
            mode_items,
            url: initial.url.clone(),
        })
    }

    fn refresh(&mut self, s: &Status) {
        self.item_status.set_text(format!(
            "Streaming {} at {}x{}",
            s.source, s.resolution.0, s.resolution.1
        ));
        self.item_clients.set_text(match s.clients {
            0 => "No clients connected".to_string(),
            1 => format!("1 client  ·  {} fps", s.fps_out),
            n => format!("{n} clients  ·  {} fps", s.fps_out),
        });
        self.item_input.set_text(input_line(s.input_enabled));
        if s.url != self.url {
            self.item_url.set_text(&s.url);
            self.url = s.url.clone();
        }
        // Keep the tick against whichever resolution is live, including after
        // a change the user did not initiate.
        for (label, (_, item)) in s.modes.iter().zip(self.mode_items.iter()) {
            item.set_checked(*label == s.current_mode);
        }
    }
}

/// Runs the menu bar app. Blocks until the user quits.
///
/// `poll` is called about twice a second on the main thread to refresh the
/// menu, so it must be cheap and must not block.
/// Runs the menu bar app. Blocks until the user quits.
///
/// `requested_mode` is how a resolution click reaches the pipeline: the menu
/// records an index and the caller's `poll` acts on it. Menu events arrive on
/// this thread, but rebuilding capture and encode is not something to do from
/// inside an event handler, so the two are decoupled by a slot.
pub fn run<F>(
    initial: Status,
    mut poll: F,
    running: Arc<AtomicBool>,
    requested_mode: Arc<std::sync::Mutex<Option<usize>>>,
) where
    F: FnMut() -> Status + 'static,
{
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("  tray UI must be started on the main thread");
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    // Accessory means a menu bar item with no Dock icon and no menu bar title,
    // which is what a background utility should be.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let mut tray = match Tray::new(&initial) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("  could not create the menu bar item: {e}");
            return;
        }
    };

    let menu_rx = MenuEvent::receiver();

    // Rather than call `app.run()`, which never returns and gives us nowhere to
    // poll from, drive the run loop manually. Each pass services AppKit for a
    // moment, then handles menu clicks and refreshes the labels.
    app.finishLaunching();

    while running.load(Ordering::SeqCst) {
        // Block up to 0.2s for the next AppKit event, then drain and dispatch
        // whatever else is queued. Running the run loop alone services timers
        // and the menu's own labels, which is why the icon draws and refreshes,
        // but a status item only pops its menu when the click is delivered
        // through NSApplication's event dispatch, which is what sendEvent: does.
        // Draining keeps the refresh cadence near 0.2s while never dropping a
        // click.
        unsafe {
            let until = NSDate::dateWithTimeIntervalSinceNow(0.2);
            let mut event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                NSDefaultRunLoopMode,
                true,
            );
            while let Some(ev) = event {
                app.sendEvent(&ev);
                event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    None,
                    NSDefaultRunLoopMode,
                    true,
                );
            }
        }

        while let Ok(event) = menu_rx.try_recv() {
            if event.id == tray.id_quit {
                running.store(false, Ordering::SeqCst);
            } else if event.id == tray.id_copy {
                copy_to_clipboard(&tray.url);
            } else if event.id == tray.id_open {
                let _ = std::process::Command::new("open").arg(&tray.url).status();
            } else if event.id == tray.id_qr {
                show_qr(&tray.url);
            } else if let Some(idx) = tray.mode_items.iter().position(|(id, _)| *id == event.id) {
                if let Ok(mut slot) = requested_mode.lock() {
                    *slot = Some(idx);
                }
            }
        }

        tray.refresh(&poll());
    }
}

/// Deliberately explicit when on. "Input: on" would be easy to skim past for
/// something that hands a remote machine your cursor and keyboard.
fn input_line(enabled: bool) -> String {
    if enabled {
        "Clients CAN control this Mac".to_string()
    } else {
        "View only".to_string()
    }
}

fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    if let Ok(mut c) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
        if let Some(mut stdin) = c.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = c.wait();
    }
}

/// Writes the QR to a temporary PNG and opens it in Preview.
///
/// Drawing it in a window of our own would mean a second AppKit surface to
/// manage for something the user looks at once, points a phone at, and closes.
fn show_qr(url: &str) {
    let Some((rgba, w, h)) = icon::qr_rgba(url, 8) else {
        return;
    };
    let path = std::env::temp_dir().join("annex-connect-qr.png");
    let Ok(file) = std::fs::File::create(&path) else {
        return;
    };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Ok(mut writer) = enc.write_header() {
        if writer.write_image_data(&rgba).is_ok() {
            drop(writer);
            let _ = std::process::Command::new("open").arg(&path).status();
        }
    }
}
