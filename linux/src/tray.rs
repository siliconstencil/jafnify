//! StatusNotifierItem tray owned by the windowless daemon. The window is a
//! separate process; closing it never stops the tunnel.

use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use jafnify_core::snapshot::{state_glyph, GlyphShape};
use jafnify_core::ConnectionState;
use ksni::blocking::{Handle, TrayMethods};

use crate::control::Request;
use crate::palette::{rgba, Colours};

/// The tray's five visible states. The shape comes from the core vocabulary;
/// this enum only says which core state the tray is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayVisual {
    Off,
    Connecting,
    Connected,
    Retrying,
    Error,
}

impl TrayVisual {
    pub fn from_state(state: ConnectionState) -> Self {
        Self::from_state_and_error(state, false)
    }

    /// An error wins over the connection state, because the tray must show it
    /// even while the tunnel is idle.
    pub fn from_state_and_error(state: ConnectionState, error: bool) -> Self {
        if error {
            return Self::Error;
        }
        match state {
            ConnectionState::Connected => Self::Connected,
            ConnectionState::Idle => Self::Off,
            ConnectionState::Retry => Self::Retrying,
            ConnectionState::Searching | ConnectionState::Pairing | ConnectionState::Connecting => {
                Self::Connecting
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Retrying => "Retrying",
            Self::Error => "Error",
        }
    }

    /// The non-colour identity. Searching and connecting share the tray's
    /// "connecting" bucket, which draws the stronger connecting variant: one
    /// arc and a filled dot.
    pub fn shape(self) -> GlyphShape {
        match self {
            Self::Off => state_glyph(ConnectionState::Idle).shape,
            Self::Connecting => state_glyph(ConnectionState::Connecting).shape,
            Self::Connected => state_glyph(ConnectionState::Connected).shape,
            Self::Retrying => state_glyph(ConnectionState::Retry).shape,
            Self::Error => GlyphShape::Exclamation,
        }
    }

    /// The tray ground is always the dark palette, so the near-white J and the
    /// state arcs stay readable on any desktop panel.
    pub fn color(self) -> [u8; 4] {
        let colours = Colours::dark();
        rgba(match self {
            Self::Off => colours.muted,
            Self::Connecting => colours.wait,
            Self::Connected => colours.ok,
            Self::Retrying => colours.wait,
            Self::Error => colours.err,
        })
    }
}

/// The label of the tray entry that copies the daemon's diagnostics text.
pub const COPY_DIAGNOSTICS_LABEL: &str = "Copy diagnostics";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    /// Open the window on one page. The daemon leaves the page hint for the
    /// window, which owns the back stack.
    OpenPage(crate::nav::Page),
    Connect,
    Disconnect,
    CopyDiagnostics,
    Exit,
}

impl TrayCommand {
    /// The control request a tray command turns into, when it needs one.
    pub fn request(self) -> Option<Request> {
        match self {
            Self::CopyDiagnostics => Some(Request::Diagnostics),
            Self::Open
            | Self::OpenPage(_)
            | Self::Connect
            | Self::Disconnect
            | Self::Exit => None,
        }
    }
}

/// Default false: the tray is the status indicator and stays visible. The
/// opt-in setting hides it only while a window is open.
pub fn tray_should_be_visible(only_when_closed: bool, window_open: bool) -> bool {
    !only_when_closed || !window_open
}

struct Tray {
    visual: TrayVisual,
    tx: mpsc::Sender<TrayCommand>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        crate::flavor::NAME.to_string()
    }

    fn title(&self) -> String {
        format!("{} - {}", crate::flavor::DISPLAY, self.visual.label())
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let size = 32;
        vec![ksni::Icon {
            width: size,
            height: size,
            data: crate::icon::state_argb(size as u32, self.visual.shape(), self.visual.color()),
        }]
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let tx = self.tx.clone();
        let open = tx.clone();
        let exit = tx.clone();
        let connected = self.visual == TrayVisual::Connected;
        let connect_tx = tx.clone();
        let diagnostics_tx = tx.clone();
        let mut items = vec![StandardItem {
            label: "Open".to_string(),
            activate: Box::new(move |_| {
                let _ = open.send(TrayCommand::Open);
            }),
            ..Default::default()
        }
        .into()];
        // Page deep links, so the tray opens the matching page in the window.
        for (page, label) in [
            (crate::nav::Page::Details, "Details"),
            (crate::nav::Page::Drops, "Drops"),
            (crate::nav::Page::Statistics, "Statistics"),
            (crate::nav::Page::Settings, "Settings"),
            (crate::nav::Page::Pairing, "Pairing"),
            (crate::nav::Page::About, "About"),
        ] {
            let page_tx = tx.clone();
            items.push(
                StandardItem {
                    label: label.to_string(),
                    activate: Box::new(move |_| {
                        let _ = page_tx.send(TrayCommand::OpenPage(page));
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(ksni::MenuItem::Separator);
        if connected {
            items.push(
                StandardItem {
                    label: "Disconnect".to_string(),
                    activate: Box::new(move |_| {
                        let _ = tx.send(TrayCommand::Disconnect);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(
                StandardItem {
                    label: "Connect".to_string(),
                    activate: Box::new(move |_| {
                        let _ = connect_tx.send(TrayCommand::Connect);
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(
            StandardItem {
                label: COPY_DIAGNOSTICS_LABEL.to_string(),
                icon_name: "edit-copy".to_string(),
                activate: Box::new(move |_| {
                    let _ = diagnostics_tx.send(TrayCommand::CopyDiagnostics);
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(ksni::MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Exit".to_string(),
                icon_name: "application-exit".to_string(),
                activate: Box::new(move |_| {
                    let _ = exit.send(TrayCommand::Exit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayCommand::Open);
    }
}

pub struct TrayHandle {
    service: Handle<Tray>,
    visual: Arc<Mutex<TrayVisual>>,
}

impl TrayHandle {
    pub fn set(&self, visual: TrayVisual) {
        if let Ok(mut current) = self.visual.lock() {
            if *current == visual {
                return;
            }
            *current = visual;
        }
        let _ = self.service.update(|tray| tray.visual = visual);
    }

    pub fn shutdown(self) {
        let _ = self.service.shutdown();
    }
}

/// The daemon owns one tray at a time. Hiding and showing the icon is a real
/// remove/add, because a hidden StatusNotifierItem is host-dependent.
#[derive(Default)]
pub struct TrayManager {
    current: Option<(TrayHandle, mpsc::Receiver<TrayCommand>)>,
    desired: bool,
    last_attempt: Option<Instant>,
}

const TRAY_RETRY: Duration = Duration::from_secs(5);

impl TrayManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn visible(&self) -> bool {
        self.current.is_some()
    }

    /// What the daemon wants, even if this desktop has no tray host. Without
    /// this, a host-less desktop would retry the D-Bus item on every tick.
    pub fn desired(&self) -> bool {
        self.desired
    }

    pub fn set_visible(&mut self, visible: bool) {
        if visible != self.desired {
            self.desired = visible;
            self.last_attempt = None;
            if !visible {
                if let Some((handle, _)) = self.current.take() {
                    handle.shutdown();
                }
                return;
            }
        }
        if self.desired && self.current.is_none() {
            let due = self
                .last_attempt
                .is_none_or(|attempt| attempt.elapsed() >= TRAY_RETRY);
            if due {
                self.last_attempt = Some(Instant::now());
                self.current = start_once();
            }
        }
    }

    pub fn set(&self, visual: TrayVisual) {
        if let Some((handle, _)) = &self.current {
            handle.set(visual);
        }
    }

    pub fn try_recv(&self) -> Option<TrayCommand> {
        self.current.as_ref().and_then(|(_, rx)| rx.try_recv().ok())
    }

    pub fn shutdown(&mut self) {
        if let Some((handle, _)) = self.current.take() {
            handle.shutdown();
        }
    }
}

fn start_once() -> Option<(TrayHandle, mpsc::Receiver<TrayCommand>)> {
    let (tx, rx) = mpsc::channel();
    let visual = Arc::new(Mutex::new(TrayVisual::Off));
    let tray = Tray {
        visual: TrayVisual::Off,
        tx,
    };
    match tray.assume_sni_available(true).spawn() {
        Ok(service) => Some((TrayHandle { service, visual }, rx)),
        Err(e) => {
            eprintln!("warning: no system tray: {e}");
            None
        }
    }
}
