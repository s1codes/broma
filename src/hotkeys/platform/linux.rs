// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Pure-Wayland Linux global hotkeys via `org.freedesktop.portal.GlobalShortcuts` (`ashpd`).
//!
//! Dispatches activated shortcuts directly to [`AudioHandle`] without bouncing
//! through the GUI state machine, and reports portal diagnostics over [`DaemonSender`].

use std::fs;
use std::io::Write;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use ashpd::AppID;
use ashpd::desktop::Session;
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use directories::BaseDirs;
use iced::futures::StreamExt;
use iced::futures::channel::mpsc;
use iced::futures::executor::block_on;

use crate::audio::service::AudioHandle;
use crate::daemon_bus::{DaemonEvent, DaemonSender};
use crate::hotkeys::{KeyToken, ParsedKeybind};
use crate::storage::SlotConfig;

/// Reverse-DNS application ID matching `ProjectDirs::from("com", "broma", "Broma")`.
pub const WAYLAND_APP_ID: &str = "io.github.s1codes.broma";

#[derive(Debug, Clone, PartialEq, Eq)]
struct WaylandShortcutEntry {
    slot_index: usize,
    shortcut_id: String,
    description: String,
    preferred_trigger: String,
}

enum PortalCommand {
    Sync(Vec<WaylandShortcutEntry>),
    Shutdown,
}

pub struct LinuxHotkeyEngine {
    audio: AudioHandle,
    bus: DaemonSender,
    desired: Vec<WaylandShortcutEntry>,
    suspended: Arc<AtomicBool>,
    worker_tx: Option<mpsc::UnboundedSender<PortalCommand>>,
    worker_join: Option<JoinHandle<()>>,
}

impl LinuxHotkeyEngine {
    pub fn new(audio: AudioHandle, bus: DaemonSender) -> Self {
        Self {
            audio,
            bus,
            desired: Vec::new(),
            suspended: Arc::new(AtomicBool::new(false)),
            worker_tx: None,
            worker_join: None,
        }
    }

    pub fn set_suspended(&mut self, suspended: bool) {
        self.suspended.store(suspended, Ordering::Relaxed);
    }

    pub fn sync_slots(&mut self, slots: &[SlotConfig], visible_count: usize) {
        let specs = crate::hotkeys::collect_visible_specs(slots, visible_count);
        let entries: Vec<WaylandShortcutEntry> = specs
            .into_iter()
            .map(|spec| WaylandShortcutEntry {
                slot_index: spec.slot_index,
                shortcut_id: format!("slot_{}", spec.slot_index),
                description: spec.label,
                preferred_trigger: to_xdg_shortcut_trigger(spec.parsed),
            })
            .collect();

        if entries == self.desired {
            return;
        }
        self.desired = entries;

        // Avoid spawning any portal D-Bus session until the user actually configures a shortcut.
        if self.desired.is_empty() && self.worker_tx.is_none() {
            return;
        }

        let snapshot = self.desired.clone();
        let tx = self.ensure_worker();
        let _ = tx.unbounded_send(PortalCommand::Sync(snapshot));
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.worker_tx.take() {
            let _ = tx.unbounded_send(PortalCommand::Shutdown);
        }
        if let Some(jh) = self.worker_join.take() {
            let _ = jh.join();
        }
    }

    fn ensure_worker(&mut self) -> &mpsc::UnboundedSender<PortalCommand> {
        if self.worker_tx.is_none() {
            let (tx, rx) = mpsc::unbounded();
            let audio = self.audio.clone();
            let bus = self.bus.clone();
            let suspended = Arc::clone(&self.suspended);

            let jh = thread::Builder::new()
                .name("broma-wayland-hotkeys".to_string())
                .spawn(move || {
                    block_on(run_portal_loop(rx, audio, bus, suspended));
                })
                .expect("spawn broma-wayland-hotkeys thread");

            self.worker_tx = Some(tx);
            self.worker_join = Some(jh);
        }
        self.worker_tx.as_ref().unwrap()
    }
}

impl Drop for LinuxHotkeyEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum LoopEvent {
    Command(PortalCommand),
    Activated(String),
}

async fn run_portal_loop(
    mut cmd_rx: mpsc::UnboundedReceiver<PortalCommand>,
    audio: AudioHandle,
    bus: DaemonSender,
    suspended: Arc<AtomicBool>,
) {
    let initial_entries = loop {
        match cmd_rx.next().await {
            Some(PortalCommand::Sync(entries)) if !entries.is_empty() => break entries,
            Some(PortalCommand::Sync(_)) => continue,
            Some(PortalCommand::Shutdown) | None => return,
        }
    };

    ensure_user_desktop_entry();

    if let Ok(app_id) = AppID::from_str(WAYLAND_APP_ID) {
        let _ = ashpd::register_host_app(app_id).await;
    }

    let portal = match GlobalShortcuts::new().await {
        Ok(p) => p,
        Err(e) => {
            bus.send(DaemonEvent::HotkeyError(format!(
                "Wayland GlobalShortcuts portal unavailable: {e}"
            )));
            return;
        }
    };

    let activated_stream = match portal.receive_activated().await {
        Ok(s) => s
            .map(|act| LoopEvent::Activated(act.shortcut_id().to_string()))
            .boxed(),
        Err(e) => {
            bus.send(DaemonEvent::HotkeyError(format!(
                "Failed to subscribe to Wayland shortcut activations: {e}"
            )));
            return;
        }
    };

    let mut active_session: Option<Session<GlobalShortcuts>> = None;
    if let Err(err) = rebind_portal_session(&portal, &mut active_session, &initial_entries).await {
        bus.send(DaemonEvent::HotkeyError(err));
    }

    let mut merged =
        iced::futures::stream::select(cmd_rx.map(LoopEvent::Command), activated_stream);

    while let Some(item) = merged.next().await {
        match item {
            LoopEvent::Command(PortalCommand::Shutdown) => {
                if let Some(session) = active_session.take() {
                    let _ = session.close().await;
                }
                break;
            }
            LoopEvent::Command(PortalCommand::Sync(entries)) => {
                if let Err(err) =
                    rebind_portal_session(&portal, &mut active_session, &entries).await
                {
                    bus.send(DaemonEvent::HotkeyError(err));
                }
            }
            LoopEvent::Activated(shortcut_id) => {
                if !suspended.load(Ordering::Relaxed)
                    && let Some(slot_idx) = parse_slot_shortcut_id(&shortcut_id)
                {
                    audio.toggle_slot(slot_idx);
                }
            }
        }
    }
}

async fn rebind_portal_session(
    portal: &GlobalShortcuts,
    active_session: &mut Option<Session<GlobalShortcuts>>,
    entries: &[WaylandShortcutEntry],
) -> Result<(), String> {
    if let Some(old_session) = active_session.take() {
        let _ = old_session.close().await;
    }

    if entries.is_empty() {
        return Ok(());
    }

    let session = portal
        .create_session(Default::default())
        .await
        .map_err(|e| format!("GlobalShortcuts CreateSession failed: {e}"))?;

    let shortcuts: Vec<NewShortcut> = entries
        .iter()
        .map(|entry| {
            NewShortcut::new(entry.shortcut_id.as_str(), entry.description.as_str())
                .preferred_trigger(Some(entry.preferred_trigger.as_str()))
        })
        .collect();

    if let Err(e) = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await
    {
        let _ = session.close().await;
        return Err(format!("GlobalShortcuts BindShortcuts failed: {e}"));
    }

    *active_session = Some(session);
    Ok(())
}

pub(crate) fn parse_slot_shortcut_id(shortcut_id: &str) -> Option<usize> {
    shortcut_id.strip_prefix("slot_")?.parse::<usize>().ok()
}

pub fn to_xdg_shortcut_trigger(kb: ParsedKeybind) -> String {
    let mut out = String::with_capacity(20);
    if kb.ctrl {
        out.push_str("CTRL+");
    }
    if kb.alt {
        out.push_str("ALT+");
    }
    if kb.shift {
        out.push_str("SHIFT+");
    }
    if kb.logo {
        out.push_str("LOGO+");
    }

    match kb.key {
        KeyToken::F(n) => out.push_str(&format!("F{n}")),
        KeyToken::Letter(ch) => out.push(ch.to_ascii_lowercase()),
        KeyToken::Digit(d) => out.push(char::from(b'0' + d)),
        KeyToken::NumpadDigit(d) => out.push_str(&format!("KP_{d}")),
        KeyToken::NumpadAdd => out.push_str("KP_Add"),
        KeyToken::NumpadSubtract => out.push_str("KP_Subtract"),
        KeyToken::NumpadMultiply => out.push_str("KP_Multiply"),
        KeyToken::NumpadDivide => out.push_str("KP_Divide"),
        KeyToken::NumpadDecimal => out.push_str("KP_Decimal"),
        KeyToken::NumpadEnter => out.push_str("KP_Enter"),
        KeyToken::Space => out.push_str("space"),
        KeyToken::Tab => out.push_str("Tab"),
        KeyToken::Enter => out.push_str("Return"),
        KeyToken::Backspace => out.push_str("BackSpace"),
        KeyToken::Delete => out.push_str("Delete"),
        KeyToken::Insert => out.push_str("Insert"),
        KeyToken::Home => out.push_str("Home"),
        KeyToken::End => out.push_str("End"),
        KeyToken::PageUp => out.push_str("Page_Up"),
        KeyToken::PageDown => out.push_str("Page_Down"),
        KeyToken::Up => out.push_str("Up"),
        KeyToken::Down => out.push_str("Down"),
        KeyToken::Left => out.push_str("Left"),
        KeyToken::Right => out.push_str("Right"),
        KeyToken::Backquote => out.push_str("grave"),
        KeyToken::Minus => out.push_str("minus"),
        KeyToken::Equal => out.push_str("equal"),
        KeyToken::BracketLeft => out.push_str("bracketleft"),
        KeyToken::BracketRight => out.push_str("bracketright"),
        KeyToken::Backslash => out.push_str("backslash"),
        KeyToken::Semicolon => out.push_str("semicolon"),
        KeyToken::Quote => out.push_str("apostrophe"),
        KeyToken::Comma => out.push_str("comma"),
        KeyToken::Period => out.push_str("period"),
        KeyToken::Slash => out.push_str("slash"),
    }

    out
}

fn ensure_user_desktop_entry() {
    let Some(base) = BaseDirs::new() else {
        return;
    };
    let apps_dir = base.data_local_dir().join("applications");
    let desktop_file = apps_dir.join(format!("{WAYLAND_APP_ID}.desktop"));
    if desktop_file.exists() {
        return;
    }

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if fs::create_dir_all(&apps_dir).is_err() {
        return;
    }

    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Broma\nComment=Low-overhead soundboard for gamers\nExec=\"{}\"\nTerminal=false\nCategories=AudioVideo;Audio;\n",
        exe.display()
    );

    let temp = apps_dir.join(format!("{WAYLAND_APP_ID}.desktop.tmp"));
    if let Ok(mut f) = fs::File::create(&temp)
        && f.write_all(content.as_bytes()).is_ok()
    {
        let _ = fs::rename(&temp, &desktop_file);
    }
}
