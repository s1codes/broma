// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Unified event bus feeding background daemon events into `iced::daemon`.
//!
//! Replaces parallel per-subsystem `Subscription` channels with a single
//! shared `UnboundedSender<DaemonEvent>` distributed at startup to the Tray,
//! Background Audio Service, and Hotkey subsystems.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use iced::Subscription;
use iced::futures::Stream;
use iced::futures::StreamExt;
use iced::futures::channel::mpsc;
use iced::stream;

#[derive(Debug, Clone)]
pub enum DaemonEvent {
    TrayToggleWindow,
    #[cfg(target_os = "windows")]
    ExternalLaunch,
    TrayQuit,
    AudioPlaybackChanged,
    AudioError(String),
    HotkeyError(String),
    DevicesRefreshed {
        microphones: Vec<String>,
        virtual_outputs: Vec<String>,
        monitor_outputs: Vec<String>,
    },
}

#[derive(Clone)]
pub struct DaemonSender {
    tx: mpsc::UnboundedSender<DaemonEvent>,
}

impl DaemonSender {
    pub fn send(&self, event: DaemonEvent) {
        let _ = self.tx.unbounded_send(event);
    }
}

#[derive(Clone)]
pub struct DaemonReceiver {
    rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<DaemonEvent>>>>,
}

impl DaemonReceiver {
    #[cfg(test)]
    pub(crate) fn take_receiver_for_test(&self) -> mpsc::UnboundedReceiver<DaemonEvent> {
        self.rx
            .lock()
            .expect("lock daemon bus rx")
            .take()
            .expect("receiver not yet taken")
    }
}

impl Hash for DaemonReceiver {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.rx).hash(state);
    }
}

pub fn create() -> (DaemonSender, DaemonReceiver) {
    let (tx, rx) = mpsc::unbounded();
    (
        DaemonSender { tx },
        DaemonReceiver {
            rx: Arc::new(Mutex::new(Some(rx))),
        },
    )
}

pub fn subscription(receiver: &DaemonReceiver) -> Subscription<DaemonEvent> {
    Subscription::run_with(receiver.clone(), build_bus_stream)
}

fn build_bus_stream(receiver: &DaemonReceiver) -> impl Stream<Item = DaemonEvent> + use<> {
    let maybe_rx = receiver.rx.lock().ok().and_then(|mut guard| guard.take());
    stream::channel(32, async move |mut output| {
        use iced::futures::SinkExt;
        let Some(mut rx) = maybe_rx else {
            return;
        };
        while let Some(evt) = rx.next().await {
            let _ = output.send(evt).await;
        }
    })
}
