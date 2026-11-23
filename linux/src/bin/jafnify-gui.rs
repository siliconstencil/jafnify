use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;
use jafnify_core::bottleneck::LinkLeg;
use jafnify_core::scope::{TileScope, TAP_TO_CHANGE};
use jafnify_core::tiles::{
    Cell, CellMark, Levels, StatusCard, Strip, Tile, Tone, Trend, TrendKind,
};
use jafnify_core::wifi_leg::WifiLeg;
use jafnify_core::ConnectionState;
use jafnify_linux::code_entry::CodeBoxes;
use jafnify_linux::connection_log;
use jafnify_linux::control::{self, InstanceLock, Request, Response, Snapshot};
use jafnify_linux::export;
use jafnify_linux::firewall::FirewallKind;
use jafnify_core::flags::Bucket;
use jafnify_linux::nav::{Nav, Page};
use jafnify_linux::palette::Colours;
use jafnify_linux::scale;
use jafnify_linux::settings::WindowGeometry;
use jafnify_linux::theme::{self, Theme, ThemeMode};
use jafnify_linux::ui_text::{format_elapsed, format_timestamp_or_none, spaced_code};
use jafnify_linux::view::{view, Primary, View, ViewInput, POLKIT_NOTE};

const DAEMON_STARTING: &str = "Starting the tray process...";

/// One label column for every Details group, wide enough for the longest
/// label ("Network switches this hour"), so the value column starts at the
/// same x in Connection, This session and Phone measurements.
const DETAIL_LABEL_WIDTH: f32 = 190.0;

const INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");

/// Install the embedded faces once. Inter Regular replaces egui's light
/// default for body and captions, Inter SemiBold is the "semibold" family for
/// big values and titles, and JetBrains Mono is the log face.
fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "inter-regular".to_owned(),
        std::sync::Arc::new(FontData::from_static(INTER_REGULAR)),
    );
    fonts.font_data.insert(
        "inter-semibold".to_owned(),
        std::sync::Arc::new(FontData::from_static(INTER_SEMIBOLD)),
    );
    fonts.font_data.insert(
        "jetbrains-mono".to_owned(),
        std::sync::Arc::new(FontData::from_static(JETBRAINS_MONO)),
    );
    let proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let monospace = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let mut regular = vec!["inter-regular".to_owned()];
    regular.extend(proportional.clone());
    fonts.families.insert(FontFamily::Proportional, regular);
    let mut mono = vec!["jetbrains-mono".to_owned()];
    mono.extend(monospace);
    fonts.families.insert(FontFamily::Monospace, mono);
    let mut semibold = vec!["inter-semibold".to_owned(), "inter-regular".to_owned()];
    semibold.extend(proportional);
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), semibold);
    ctx.set_fonts(fonts);
}

fn semibold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("semibold".into()))
}

fn uppercase_label(ui: &mut egui::Ui, text: &str, size: f32, colour: egui::Color32) {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        egui::FontId::proportional(size),
        colour,
        ui.available_width(),
    );
    for section in &mut job.sections {
        section.format.extra_letter_spacing = 0.5;
    }
    ui.add(egui::Label::new(job));
}

fn main() -> eframe::Result<()> {
    let lock_path = match control::window_lock_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("jafnify: {e}");
            return Ok(());
        }
    };
    let window_lock = match InstanceLock::acquire(&lock_path) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Ok(()),
        Err(e) => {
            eprintln!("jafnify: cannot lock the window: {e}");
            return Ok(());
        }
    };
    if let Err(e) = control::ensure_daemon() {
        eprintln!("jafnify: cannot start the tray process: {e}");
        return Ok(());
    }
    let initial = match control::request(&Request::GetState) {
        Ok(Response::State(snapshot)) => Some(*snapshot),
        _ => None,
    };
    let saved = initial.as_ref().and_then(|s| s.window.sanitized());
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(scale::WINDOW_SIZE)
        .with_min_inner_size(scale::WINDOW_SIZE)
        .with_max_inner_size(scale::WINDOW_SIZE)
        .with_resizable(false)
        .with_maximize_button(false)
        .with_title(jafnify_linux::flavor::DISPLAY)
        .with_app_id(jafnify_linux::flavor::NAME)
        .with_icon(std::sync::Arc::new(jafnify_linux::icon::icon_data(64)));
    if let Some(window) = saved {
        // The size is fixed; only the position is restored, and the first
        // frame clamps it onto the monitor.
        if let (Some(x), Some(y)) = (window.x, window.y) {
            viewport = viewport.with_position([x as f32, y as f32]);
        }
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        jafnify_linux::flavor::DISPLAY,
        options,
        Box::new(move |cc| {
            // The window is fixed: Ctrl+Plus/Minus must not scale the UI.
            cc.egui_ctx
                .options_mut(|options| options.zoom_with_keyboard = false);
            // The layout is the design box at the fixed zoom from the first
            // frame, even where the monitor size is never reported.
            cc.egui_ctx.set_zoom_factor(scale::ZOOM);
            Ok(Box::new(Window::new(window_lock, initial)))
        }),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm {
    Forget,
    PairNew,
    AllowFirewall,
    Terms,
    Stop,
    ClearDrops,
    ClearGoodHours,
}

enum Action {
    Primary(Primary),
    SubmitCode,
    PairNew,
    Forget,
    CancelEntry,
    AllowFirewall,
    /// The Stop button was pressed: ask first, then disconnect.
    StopRequested,
    /// A tile was tapped: its scope moves on.
    TileScope(String),
}

/// A size/position must stay unchanged this long before it is saved.
const GEOMETRY_SETTLE: Duration = Duration::from_millis(800);
/// How many drops the window shows in Details.
const DROP_ROWS_SHOWN: usize = 5;
/// How often the window reads the drop history from the daemon.
const DROPS_POLL_INTERVAL: Duration = Duration::from_secs(2);

struct Window {
    _lock: InstanceLock,
    /// Last geometry written to the daemon (or loaded at start).
    geometry_saved: Option<WindowGeometry>,
    /// A changed geometry and when it was first seen, waiting to settle.
    geometry_pending: Option<(WindowGeometry, Instant)>,
    /// The page on screen and its back stack.
    nav: Nav,
    /// The floored text sizes for this frame's pixels_per_point.
    text: scale::TextSizes,
    /// The text sizes last written into the egui style.
    last_text: Option<scale::TextSizes>,
    /// A saved outer position waiting to be clamped onto the monitor.
    pending_position: Option<egui::Pos2>,
    /// The one-time start-up fit has been decided.
    startup_zoom_applied: bool,
    /// The one-time start-up zoom actually applied (1.0 normally).
    applied_zoom: f32,
    /// The embedded fonts have been installed.
    fonts_loaded: bool,
    /// True while the About modal covers the page.
    about_open: bool,
    snapshot: Snapshot,
    got_snapshot: bool,
    terms_checked: bool,
    terms_ok: bool,
    snapshot_error: Option<String>,
    boxes: CodeBoxes,
    pairing_new: bool,
    confirm: Option<Confirm>,
    start_error: Option<String>,
    code_error: Option<String>,
    entry_shown: bool,
    focus_first: bool,
    settings_loaded: bool,
    role_tmp: String,
    autostart_tmp: bool,
    kill_tmp: bool,
    tray_tmp: bool,
    theme_tmp: ThemeMode,
    nickname_tmp: String,
    /// A running link test, off the UI thread.
    link_test_rx: Option<std::sync::mpsc::Receiver<(String, Option<LinkLeg>)>>,
    link_test_result: Option<LinkLeg>,
    /// The last link-test line, shown while the flag is ON.
    link_test_line: Option<String>,
    /// Read once at window start; not re-detected every frame.
    portal_color_scheme: Option<u32>,
    kdeglobals: PathBuf,
    /// Resolved once per selected mode; the portal value is fixed at start.
    cached_theme: Option<(ThemeMode, Theme)>,
    last_applied_theme: Option<Theme>,
    settings_note: Option<String>,
    firewall_note: Option<String>,
    details_note: Option<String>,
    log_note: Option<String>,
    closed_sent: bool,
    daemon_gone: bool,
    /// The window mark, rendered once from the shared SVG and reused.
    brand_tex: Option<egui::TextureHandle>,
    /// The drop history of the last day, read from the daemon.
    drops: Option<DropsView>,
    /// Why the drop history could not be read, if it could not.
    drops_error: Option<String>,
    /// When the drop history was read last.
    drops_read_at: Option<Instant>,
}

/// The drop history as the daemon sends it: the summary of the last day and
/// the newest drops, newest first.
struct DropsView {
    summary: String,
    rows: Vec<String>,
}

impl Window {
    fn new(lock: InstanceLock, initial: Option<Snapshot>) -> Self {
        let got_snapshot = initial.is_some();
        let snapshot = initial.unwrap_or_else(|| Snapshot {
            state: "idle".to_string(),
            status_text: DAEMON_STARTING.to_string(),
            ..Snapshot::default()
        });
        let terms_ok = snapshot.terms_ok || !got_snapshot;
        let theme_tmp = ThemeMode::parse(&snapshot.theme).unwrap_or_default();
        let portal_color_scheme = theme::detect_portal_color_scheme();
        let kdeglobals = theme::kdeglobals_path();
        let pending_position = snapshot
            .window
            .x
            .zip(snapshot.window.y)
            .map(|(x, y)| egui::pos2(x as f32, y as f32));
        let mut window = Self {
            _lock: lock,
            geometry_saved: None,
            geometry_pending: None,
            nav: Nav::new(),
            text: scale::TextSizes::new(1.0),
            last_text: None,
            pending_position,
            startup_zoom_applied: false,
            applied_zoom: scale::ZOOM,
            fonts_loaded: false,
            about_open: false,
            snapshot,
            got_snapshot,
            terms_checked: got_snapshot,
            terms_ok,
            snapshot_error: None,
            boxes: CodeBoxes::default(),
            pairing_new: false,
            confirm: None,
            start_error: None,
            code_error: None,
            entry_shown: false,
            focus_first: false,
            settings_loaded: false,
            role_tmp: "joiner".to_string(),
            autostart_tmp: false,
            kill_tmp: false,
            tray_tmp: false,
            theme_tmp,
            nickname_tmp: String::new(),
            link_test_rx: None,
            link_test_result: None,
            link_test_line: None,
            portal_color_scheme,
            kdeglobals,
            cached_theme: None,
            last_applied_theme: None,
            settings_note: None,
            firewall_note: None,
            details_note: None,
            log_note: None,
            closed_sent: false,
            daemon_gone: false,
            brand_tex: None,
            drops: None,
            drops_error: None,
            drops_read_at: None,
        };
        if got_snapshot && !terms_ok {
            window.confirm = Some(Confirm::Terms);
        }
        if got_snapshot {
            window.load_settings();
        }
        window
    }

    fn load_settings(&mut self) {
        if self.settings_loaded {
            return;
        }
        self.settings_loaded = true;
        self.role_tmp = self.snapshot.role.clone();
        self.autostart_tmp = self.snapshot.autostart;
        self.kill_tmp = self.snapshot.kill_switch;
        self.tray_tmp = self.snapshot.tray_only_when_closed;
        self.theme_tmp = ThemeMode::parse(&self.snapshot.theme).unwrap_or_default();
        self.nickname_tmp = self.snapshot.nickname.clone();
    }

    fn poll(&mut self) {
        match control::request(&Request::GetState) {
            Ok(Response::State(snapshot)) => {
                self.snapshot = *snapshot;
                self.got_snapshot = true;
                self.terms_ok = self.snapshot.terms_ok;
                if !self.settings_loaded {
                    self.load_settings();
                }
                if state_from(&self.snapshot.state) == ConnectionState::Connected {
                    self.pairing_new = false;
                    self.start_error = None;
                    self.boxes.clear();
                } else {
                    self.link_test_result = None;
                    self.link_test_line = None;
                }
                if !self.terms_checked {
                    self.terms_checked = true;
                    if !self.snapshot.terms_ok {
                        self.confirm = Some(Confirm::Terms);
                    }
                }
                self.snapshot_error = None;
            }
            Ok(Response::Err { message }) => self.snapshot_error = Some(message),
            Ok(Response::Ok { .. }) => {}
            Ok(Response::Page { .. }) => {}
            Ok(Response::Diagnostics { .. }) => {}
            Ok(Response::LinkTest { .. }) => {}
            Ok(Response::Drops { .. }) => {}
            Ok(Response::BestSpot {
                running,
                active,
                lines,
            }) => {
                self.snapshot.best_spot_running = running;
                self.snapshot.best_spot_active = active;
                self.snapshot.best_spot_lines = lines;
            }
            Ok(Response::GoodHours { lines }) => {
                self.snapshot.good_hours_lines = lines;
            }
            Err(control::ControlError::Timeout) => {}
            Err(control::ControlError::Io(e)) => {
                // The tray process ended (tray Exit stops it). The window is
                // modal to that process, so it closes too.
                self.daemon_gone = true;
                self.snapshot_error = Some(e.to_string());
            }
            Err(control::ControlError::NoRuntimeDir) => {
                self.daemon_gone = true;
                self.snapshot_error = Some("XDG_RUNTIME_DIR is not set".to_string());
            }
            Err(e) => self.snapshot_error = Some(e.to_string()),
        }
    }

    /// Pick up a page hint left by a tray entry. The daemon owns the tray, so
    /// the window asks for the pending page on every poll; an unknown name is
    /// ignored by [Page::parse] rather than opening a wrong page.
    fn poll_page_hint(&mut self) {
        if let Ok(Response::Page { page }) = control::request(&Request::TakeOpenPage) {
            if let Some(page) = page.as_deref().and_then(Page::parse) {
                self.open_page(page);
            }
        }
    }

    fn send(&mut self, request: Request) -> Result<Option<String>, String> {
        match control::request(&request) {
            Ok(Response::Ok { note }) => Ok(note),
            Ok(Response::Err { message }) => Err(message),
            Ok(Response::State(snapshot)) => {
                self.snapshot = *snapshot;
                if state_from(&self.snapshot.state) != ConnectionState::Connected {
                    self.link_test_result = None;
                    self.link_test_line = None;
                }
                Ok(None)
            }
            Ok(Response::Diagnostics { text }) => Ok(Some(text)),
            Ok(Response::LinkTest { line, .. }) => Ok(Some(line)),
            Ok(Response::Drops { summary, rows }) => {
                self.drops = Some(DropsView { summary, rows });
                Ok(None)
            }
            Ok(Response::BestSpot {
                running,
                active,
                lines,
            }) => {
                self.snapshot.best_spot_running = running;
                self.snapshot.best_spot_active = active;
                self.snapshot.best_spot_lines = lines;
                Ok(None)
            }
            Ok(Response::GoodHours { lines }) => {
                self.snapshot.good_hours_lines = lines;
                Ok(None)
            }
            Ok(Response::Page { .. }) => Ok(None),
            // A password prompt is open: the command will finish in the
            // daemon; the next poll shows the result.
            Err(control::ControlError::Timeout) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn input(&self) -> ViewInput {
        let mut input = ViewInput::new(state_from(&self.snapshot.state));
        input.app_name = jafnify_linux::flavor::DISPLAY;
        input.paired_name = self.snapshot.paired_name.clone();
        input.denied = self.snapshot.denied;
        input.error = self
            .start_error
            .clone()
            .or_else(|| self.snapshot.last_error.clone())
            .or_else(|| self.snapshot_error.clone());
        input.retry_reason = self.snapshot.retry_reason.clone();
        input.no_peer = self.snapshot.no_peer;
        input.pairing_new = self.pairing_new;
        input.code_digits = self.boxes.len();
        input.connected_secs = self.snapshot.connected_secs;
        input.traffic = self.snapshot.traffic;
        input.last_connected_ago = self.snapshot.last_connected_ago;
        input.retry_secs_left = self.snapshot.retry_secs_left;
        input.firewall = match self.snapshot.firewall.as_deref() {
            Some("ufw") => Some(FirewallKind::Ufw),
            Some("other") => Some(FirewallKind::Other),
            _ => None,
        };
        input.firewall_iface = self.snapshot.firewall_iface.clone();
        input.polkit_note = self.snapshot.polkit_note;
        input.link_lost = self.snapshot.link_lost;
        input.no_network = self.snapshot.no_network;
        input.health = self.snapshot.health_line.clone();
        input.foreign_tunnel = self.snapshot.foreign_tunnel;
        input.code_expired = self.snapshot.code_expired;
        input
    }

    fn handle(&mut self, action: Action) {
        let starts = matches!(
            action,
            Action::Primary(Primary::Connect)
                | Action::Primary(Primary::TryAgain)
                | Action::SubmitCode
        );
        if starts && !self.terms_ok {
            self.confirm = Some(Confirm::Terms);
            return;
        }
        match action {
            Action::Primary(Primary::Connect) | Action::SubmitCode => {
                self.code_error = None;
                if self.pairing_new || !self.snapshot.paired {
                    match self.send(Request::Pair {
                        code: self.boxes.code(),
                    }) {
                        Ok(_) => {}
                        Err(e) => {
                            if e.contains("invalid pairing code") {
                                self.code_error = Some(
                                    "Invalid code - enter the 4 digits shown on the phone"
                                        .to_string(),
                                );
                                self.boxes.clear();
                                self.focus_first = true;
                            } else {
                                self.start_error = Some(e);
                            }
                        }
                    }
                } else if let Err(e) = self.send(Request::Connect) {
                    self.start_error = Some(e);
                } else {
                    self.start_error = None;
                }
            }
            Action::Primary(Primary::TryAgain) => {
                if let Err(e) = self.send(Request::TryAgain) {
                    self.start_error = Some(e);
                }
            }
            Action::Primary(Primary::Cancel) | Action::Primary(Primary::Disconnect) => {
                if let Err(e) = self.send(Request::Disconnect) {
                    self.start_error = Some(e);
                }
                self.pairing_new = false;
            }
            Action::PairNew => self.confirm = Some(Confirm::PairNew),
            Action::Forget => self.confirm = Some(Confirm::Forget),
            Action::AllowFirewall => self.confirm = Some(Confirm::AllowFirewall),
            Action::StopRequested => self.confirm = Some(Confirm::Stop),
            Action::CancelEntry => {
                self.pairing_new = false;
                self.boxes.clear();
                self.code_error = None;
            }
            Action::TileScope(id) => {
                // The core owns the cycle: the window only asks for the next
                // scope of the tile and hands it to the daemon.
                let scope = self.next_tile_scope(&id);
                if let Err(e) = self.send(Request::SetTileScope {
                    id,
                    scope: scope.word().to_string(),
                }) {
                    self.start_error = Some(e);
                }
            }
        }
    }

    fn answer(&mut self, confirm: Confirm) {
        match confirm {
            Confirm::Terms => match self.send(Request::AcceptTerms) {
                Ok(_) => {
                    self.terms_ok = true;
                    self.start_error = None;
                }
                Err(e) => self.start_error = Some(format!("Could not save the acceptance: {e}")),
            },
            Confirm::Forget => match self.send(Request::Forget) {
                Ok(_) => {
                    self.start_error = None;
                    self.pairing_new = false;
                    self.boxes.clear();
                }
                Err(e) => self.start_error = Some(e),
            },
            Confirm::PairNew => {
                if state_from(&self.snapshot.state) != ConnectionState::Idle {
                    let _ = self.send(Request::Disconnect);
                }
                self.pairing_new = true;
                self.boxes.clear();
                self.focus_first = true;
            }
            Confirm::AllowFirewall => {
                self.firewall_note = Some(match self.send(Request::AllowFirewall) {
                    Ok(Some(note)) => note,
                    Ok(None) => "Waiting for the password prompt...".to_string(),
                    Err(e) => e,
                });
            }
            Confirm::Stop => {
                if let Err(e) = self.send(Request::Disconnect) {
                    self.start_error = Some(e);
                }
                self.pairing_new = false;
            }
            Confirm::ClearDrops => {
                self.details_note = match self.send(Request::ClearDrops) {
                    Ok(Some(note)) => Some(note),
                    Ok(None) => Some("Drop history cleared.".to_string()),
                    Err(e) => Some(format!("Could not clear the drop history: {e}")),
                };
                self.read_drops();
            }
            Confirm::ClearGoodHours => {
                self.details_note = match self.send(Request::ClearGoodHours) {
                    Ok(Some(note)) => Some(note),
                    Ok(None) => Some("Good hours cleared.".to_string()),
                    Err(e) => Some(format!("Could not clear the good hours: {e}")),
                };
            }
        }
    }

    fn code_boxes(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        let mut focus = std::mem::take(&mut self.focus_first).then_some(0);
        let mut changed = false;

        let pasted = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Paste(s) => Some(s.clone()),
                _ => None,
            })
        });
        if let Some(text) = pasted {
            ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Paste(_))));
            if let Some(n) = self.boxes.paste(&text) {
                focus = Some(n);
                changed = true;
            }
        }
        let backspace = ctx.input(|i| i.key_pressed(egui::Key::Backspace));
        let box_ids: [egui::Id; 4] =
            std::array::from_fn(|i| ui.make_persistent_id(("code_box", i)));

        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - 184.0) / 2.0).max(0.0));
            for (i, box_id) in box_ids.iter().enumerate() {
                let mut text = self.boxes.text(i).to_string();
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .id(*box_id)
                        .desired_width(36.0)
                        .align(egui::Align2::CENTER_CENTER)
                        .font(egui::FontId::proportional(self.text.scale(26.0))),
                );
                if resp.changed() {
                    focus = self.boxes.input(i, &text).or(focus);
                    changed = true;
                } else if resp.has_focus() && backspace && self.boxes.text(i).is_empty() {
                    focus = self.boxes.backspace(i).or(focus);
                }
            }
        });
        if let Some(n) = focus {
            ctx.memory_mut(|m| m.request_focus(box_ids[n]));
            ctx.request_repaint();
        }
        changed && self.boxes.is_complete()
    }

    fn colours(&self) -> Colours {
        let theme = self
            .cached_theme
            .map(|(_, theme)| theme)
            .unwrap_or(Theme::Dark);
        Colours::for_theme(theme)
    }

    /// The theme in force: the cached resolved theme, else the dark default.
    fn resolved_theme(&self) -> Theme {
        self.cached_theme
            .map(|(_, theme)| theme)
            .unwrap_or(Theme::Dark)
    }

    /// Recompute the floored text sizes for this frame and publish them to the
    /// egui style, so implicit labels, buttons and monospace text use the same
    /// physical floor as the explicit sizes. The style is written only when
    /// the sizes change.
    fn apply_text(&mut self, ctx: &egui::Context, ppp: f32) {
        let text = scale::TextSizes::new(ppp);
        self.text = text;
        if self.last_text == Some(text) {
            return;
        }
        self.last_text = Some(text);
        ctx.all_styles_mut(|style| {
            use egui::TextStyle;
            style.text_styles.insert(
                TextStyle::Small,
                egui::FontId::proportional(text.caption),
            );
            style
                .text_styles
                .insert(TextStyle::Body, egui::FontId::proportional(text.body));
            style
                .text_styles
                .insert(TextStyle::Button, egui::FontId::proportional(text.body));
            style.text_styles.insert(TextStyle::Heading, semibold(text.title));
            style
                .text_styles
                .insert(TextStyle::Monospace, egui::FontId::monospace(text.body));
        });
    }

    /// Open a page and keep the daemon's probe gate in step: the host hints
    /// are only read while Details is the page on screen.
    fn open_page(&mut self, page: Page) {
        // About is the one short surface that is a modal, not a page: every
        // entry point (header, Settings, tray) uses the same modal.
        if page == Page::About {
            self.about_open = true;
            return;
        }
        self.nav.open(page);
        self.sync_view_state();
    }

    /// Back one page (or to the home). Same gate update as [Self::open_page].
    fn go_back(&mut self) {
        self.nav.back();
        self.sync_view_state();
    }

    fn sync_view_state(&mut self) {
        let on_details = self.nav.current() == Page::Details;
        if self.snapshot.details_expanded != on_details {
            self.snapshot.details_expanded = on_details;
            self.persist_view_state();
        }
    }

    /// Render the window mark once from the shared SVG source.
    fn ensure_brand_texture(&mut self, ctx: &egui::Context) {
        if self.brand_tex.is_some() {
            return;
        }
        const SIZE: u32 = 64;
        let rgba = jafnify_linux::icon::rgba(SIZE);
        let image = egui::ColorImage::from_rgba_unmultiplied([SIZE as usize, SIZE as usize], &rgba);
        self.brand_tex =
            Some(ctx.load_texture("jafnify-mark", image, egui::TextureOptions::LINEAR));
    }

    /// The brand header above the status card: mark plus the wordmark. The
    /// whole row is the About target; clicking it opens the About modal.
    fn brand_header(&self, ui: &mut egui::Ui, texture: &egui::TextureHandle) -> bool {
        let metrics = theme::metrics();
        let row = ui.horizontal(|ui| {
            ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(28.0, 28.0)));
            ui.add_space(metrics.spacing.sm);
            ui.label(
                egui::RichText::new(jafnify_linux::flavor::DISPLAY)
                    .font(semibold(self.text.title)),
            );
        });
        let response = ui
            .interact(
                row.response.rect,
                egui::Id::unique("jafnify-about"),
                egui::Sense::click(),
            )
            .on_hover_text("About Jafnify");
        response.clicked()
    }

    fn card(&self, ui: &mut egui::Ui, v: &View) {
        let resolved = self
            .cached_theme
            .map(|(_, theme)| theme)
            .unwrap_or(Theme::Dark);
        let colours = Colours::for_theme(resolved);
        let metrics = theme::metrics();
        resolved.card_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(&v.title).font(semibold(self.text.title)));
                if let Some(intro) = &v.intro {
                    ui.add_space(metrics.spacing.sm);
                    ui.label(intro);
                }
                ui.add_space(metrics.spacing.sm);
                ui.horizontal_wrapped(|ui| {
                    // The status text keeps the body colour: the `wait` amber
                    // does not reach 4.5:1 in the light theme. The state only
                    // colours the small mark in front, and errors also get a
                    // `!` so they are never colour alone.
                    let tone = if v.warning {
                        colours.err
                    } else {
                        match v.state {
                            ConnectionState::Idle => colours.muted,
                            ConnectionState::Connected => colours.ok,
                            _ => colours.wait,
                        }
                    };
                    let prefix = if v.warning { "! " } else { "* " };
                    ui.label(egui::RichText::new(prefix).color(tone));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&v.status)
                                .size(self.text.body_large)
                                .color(colours.fg),
                        )
                        .wrap(),
                    );
                });
                if let Some(health) = &v.health {
                    ui.label(egui::RichText::new(health).color(colours.muted));
                }
                if let Some(extra) = &v.extra {
                    ui.label(extra);
                }
                if let Some(note) = &v.kill_switch_note {
                    ui.label(egui::RichText::new(note).color(colours.muted));
                }
                if let Some(help) = &v.help {
                    ui.add_space(metrics.spacing.xs);
                    ui.label(help);
                    if let Some(note) = &self.firewall_note {
                        ui.label(note);
                    }
                }
                for line in [&v.elapsed, &v.traffic, &v.last].into_iter().flatten() {
                    ui.label(line);
                }
            });
        });
    }

    /// True while the tile home is on.
    fn tile_home_on(&self) -> bool {
        self.snapshot
            .flags
            .get("tile_home")
            .copied()
            .unwrap_or(false)
            // Without the home the daemon did not send, the window stays the
            // old one: the card and the button must never disappear.
            && self.snapshot.home.is_some()
    }

    /// The home screen as instrument tiles: the status card and the chosen
    /// grid, nothing below it. The usual hours and the page buttons are not
    /// part of the fixed home. The daemon owns the data; the window draws it.
    fn tile_home(
        &mut self,
        ui: &mut egui::Ui,
        colours: &Colours,
        v: &View,
        actions: &mut Vec<Action>,
    ) {
        let Some(home) = self.snapshot.home.clone() else {
            return;
        };
        let metrics = theme::metrics();
        self.tile_status_card(ui, colours, &home.status, v, actions);
        ui.add_space(metrics.spacing.md);
        let button_height = 44.0;
        let gap = metrics.spacing.md;
        // The tile rows share every point between the card and the bottom
        // row, so the fixed home has no empty band under the buttons.
        let grid_fill = (ui.available_height() - button_height - gap).max(0.0);
        if !home.tiles.is_empty() {
            self.tile_grid(ui, colours, &home.tiles, actions, Some(grid_fill));
        }
        let remaining = ui.available_height();
        if remaining > button_height {
            ui.add_space(remaining - button_height);
        }
        self.tile_buttons(ui);
    }

    /// The bottom row of the fixed home: one page button each, the same order
    /// as the phone's Best spot / This phone / Statistics row plus Settings.
    fn tile_buttons(&mut self, ui: &mut egui::Ui) {
        let text = self.text;
        let metrics = theme::metrics();
        let gap = metrics.spacing.sm;
        let mut pages: Vec<(Page, &'static str)> = Vec::new();
        if self.best_spot_on() {
            pages.push((Page::BestSpot, "Best spot"));
        }
        pages.push((Page::Details, "Details"));
        pages.push((Page::Statistics, "Statistics"));
        pages.push((Page::Settings, "Settings"));
        let count = pages.len() as f32;
        let each = ((ui.available_width() - gap * (count - 1.0)) / count).max(80.0);
        let mut open = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (page, label) in pages {
                let button = egui::Button::new(egui::RichText::new(label).size(text.body))
                    .min_size(egui::vec2(each, 44.0));
                if ui.add(button).clicked() {
                    open = Some(page);
                }
            }
        });
        if let Some(page) = open {
            self.open_page(page);
        }
    }

    /// The status card at the top: the mark, the name, the path and the time,
    /// the good-time chip and the one button.
    fn tile_status_card(
        &self,
        ui: &mut egui::Ui,
        colours: &Colours,
        status: &StatusCard,
        v: &View,
        actions: &mut Vec<Action>,
    ) {
        let metrics = theme::metrics();
        let resolved = self.resolved_theme();
        let mut frame = resolved.card_frame();
        frame.inner_margin = egui::Margin::same(metrics.spacing.md as i8);
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().circle_filled(
                    dot.center(),
                    5.0,
                    if status.connected {
                        colours.ok
                    } else {
                        colours.muted
                    },
                );
                ui.add_space(metrics.spacing.sm);
                ui.label(egui::RichText::new(&status.title).font(semibold(self.text.title)));
            });
            if let Some(subline) = &status.subline {
                ui.label(
                    egui::RichText::new(subline)
                        .size(self.text.body)
                        .color(colours.muted),
                );
            }
            if let Some(notice) = &status.notice {
                ui.add_space(metrics.spacing.sm);
                egui::Frame {
                    inner_margin: egui::Margin::symmetric(
                        metrics.spacing.sm as i8,
                        metrics.spacing.xs as i8,
                    ),
                    corner_radius: egui::CornerRadius::same(metrics.radius.sm as u8),
                    fill: colours.surface,
                    stroke: egui::Stroke::new(1.0, colours.ok),
                    ..Default::default()
                }
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(notice)
                            .size(self.text.caption)
                            .color(colours.ok),
                    );
                });
            }
            ui.add_space(metrics.spacing.md);
            // The one button: Stop while the tunnel is up, the usual action
            // otherwise. The button is never smaller than a finger.
            let label = if status.stop {
                "Stop".to_string()
            } else {
                v.primary.label().to_string()
            };
            let button =
                egui::Button::new(egui::RichText::new(label).size(self.text.body_large))
                    .min_size(egui::vec2(160.0, 44.0));
            if status.stop {
                if ui.add_enabled(v.primary_enabled, button).clicked() {
                    actions.push(Action::StopRequested);
                }
            } else if ui.add_enabled(v.primary_enabled, button).clicked() {
                actions.push(Action::Primary(v.primary));
            }
        });
    }

    /// The tiles in two columns. A 7 grid has one wide tile, at the top or at
    /// the bottom: it takes the whole width and the other six keep two
    /// columns. The order and the wide slot come from the saved layout.
    fn tile_grid(
        &self,
        ui: &mut egui::Ui,
        colours: &Colours,
        tiles: &[Tile],
        actions: &mut Vec<Action>,
        fill_height: Option<f32>,
    ) {
        let metrics = theme::metrics();
        let gap = metrics.spacing.sm;
        // One height for every row of every grid: the tiles of a row line up,
        // AVERAGE is as tall as its neighbour, and a wide 7-grid tile keeps
        // the same height as the two tiles it spans.
        let full = ui.available_width();
        let layout = jafnify_core::home_layout::Layout::parse(&self.snapshot.home_layout)
            .unwrap_or_else(jafnify_linux::home_layout::default_layout)
            .normalize(jafnify_core::home_layout::Platform::Linux);
        let rows = jafnify_linux::home_layout::row_plans(&layout, tiles.len());
        // The row height follows the tallest tile on the current font metrics,
        // so every row has one height and no tile can grow past it.
        let caption_row = ui.ctx().fonts_mut(|fonts| {
            fonts.row_height(&egui::FontId::proportional(self.text.caption))
        });
        let value_row = ui
            .ctx()
            .fonts_mut(|fonts| fonts.row_height(&semibold(self.text.title)));
        let needs: Vec<f32> = tiles
            .iter()
            .map(|tile| {
                let facts = tile.facts.as_ref().map_or(0, |facts| facts.lines.len());
                let lines = facts + usize::from(tile.note.is_some());
                jafnify_linux::home_layout::tile_height(
                    caption_row,
                    value_row,
                    metrics.line_height.caption,
                    lines,
                    metrics.spacing.xs,
                    ui.spacing().item_spacing.y,
                    metrics.spacing.md,
                )
            })
            .collect();
        let natural = jafnify_linux::home_layout::grid_row_height(&needs);
        let row_count = rows.len() as f32;
        // In the fixed window the rows share the space above the bottom row
        // and never take more, so the page buttons always stay inside it.
        let row_height = match fill_height {
            Some(fill) if row_count > 0.0 => {
                ((fill - gap * (row_count - 1.0)) / row_count).max(0.0)
            }
            _ => natural,
        };
        let mut first = true;
        for row in rows {
            if !first {
                ui.add_space((gap - ui.spacing().item_spacing.y).max(0.0));
            }
            first = false;
            let width = jafnify_linux::home_layout::row_width(row.wide, full, gap);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for tile in &tiles[row.start..row.end] {
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, row_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(width);
                            ui.set_height(row_height);
                            self.tile_card(ui, colours, tile, row_height, actions);
                        },
                    );
                }
            });
        }
    }

    /// One tile: the label, the badge, the number, the drawing and the facts.
    /// The whole tile is the target of the tap: a tile that counts over time
    /// changes its scope when it is tapped.
    fn tile_card(
        &self,
        ui: &mut egui::Ui,
        colours: &Colours,
        tile: &Tile,
        row_height: f32,
        actions: &mut Vec<Action>,
    ) {
        let metrics = theme::metrics();
        let resolved = self.resolved_theme();
        // Every tile, AVERAGE included, takes the same frame and the same
        // fixed inner height: the row height is the tile height.
        let mut frame = resolved.card_frame();
        frame.inner_margin = egui::Margin::same(metrics.spacing.md as i8);
        let inner_height = row_height - 2.0 * metrics.spacing.md;
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(inner_height);
            ui.horizontal(|ui| {
                uppercase_label(ui, &tile.label, self.text.caption, colours.muted);
                if let Some(badge) = &tile.badge {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(&badge.text)
                                .size(self.text.caption)
                                .color(tone_colour(colours, badge.tone))
                                .strong(),
                        );
                    });
                }
            });
            if let Some(big) = &tile.big {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(&big.value)
                            .font(semibold(self.text.title))
                            .color(tone_colour(colours, big.tone)),
                    );
                    if let Some(unit) = &big.unit {
                        ui.label(
                            egui::RichText::new(unit)
                                .size(self.text.body)
                                .color(colours.muted),
                        );
                    }
                    if let Some(levels) = tile.trend.as_ref().and_then(|trend| trend.levels) {
                        ui.add_space(metrics.spacing.sm);
                        tile_levels(ui, colours, &levels);
                    }
                });
            }
            if let Some(trend) = &tile.trend {
                ui.add_space(metrics.spacing.xs);
                tile_trend(ui, colours, trend);
            }
            if let Some(facts) = &tile.facts {
                ui.add_space(metrics.spacing.xs);
                // A fact is one line: a long one ends in an ellipsis and
                // shows in full on hover, so no tile grows past its row.
                for line in &facts.lines {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(line)
                                .size(self.text.caption)
                                .color(colours.muted),
                        )
                        .truncate(),
                    );
                }
            }
            if let Some(note) = &tile.note {
                ui.add_space(metrics.spacing.xs);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(note)
                            .size(self.text.caption)
                            .color(colours.muted),
                    )
                    .truncate(),
                );
            }
            if TileScope::is_scopeable(&tile.id) {
                let response = ui.interact(
                    ui.max_rect(),
                    ui.scope_id().with(("tile_scope", tile.id.as_str())),
                    egui::Sense::click(),
                );
                if response.clicked() {
                    actions.push(Action::TileScope(tile.id.clone()));
                }
                response.on_hover_text(TAP_TO_CHANGE);
            }
        });
    }

    /// The scope a tile moves to when it is tapped. The core owns the order of
    /// the cycle; the window only reads the choice and asks for the next one.
    fn next_tile_scope(&self, tile_id: &str) -> TileScope {
        let current = TileScope::for_tile(
            tile_id,
            self.snapshot.tile_scopes.get(tile_id).map(String::as_str),
        );
        current.next(tile_id)
    }

    /// The strip of the hours of the day: the quick and the busy hours. The
    /// page draws one heading of its own, so the strip's title can be left
    /// out where the page already named the section.
    fn hours_strip(
        &self,
        ui: &mut egui::Ui,
        colours: &Colours,
        strip: &Strip,
        show_title: bool,
    ) {
        let metrics = theme::metrics();
        if show_title {
            uppercase_label(ui, &strip.title, self.text.caption, colours.muted);
        }
        // The caption says why the strip looks the way it does: the learning
        // line, or the line that says no hour has a verdict yet. It is above
        // the strip, where it is always seen.
        let caption = strip.caption.as_ref().or(strip.learning.as_ref());
        if let Some(caption) = caption {
            ui.add_space(metrics.spacing.xs);
            ui.label(
                egui::RichText::new(caption)
                    .size(self.text.caption)
                    .color(colours.muted),
            );
        }
        ui.add_space(metrics.spacing.xs);
        let width = ui.available_width();
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width, HOURS_STRIP_HEIGHT), egui::Sense::hover());
        hour_cells(ui, colours, &strip.cells, rect);
        // The hour ticks under the strip, spread across its width: the left
        // edge is hour 0, the right edge is hour 24.
        let (ticks, _) = ui.allocate_exact_size(
            egui::vec2(width, metrics.line_height.caption),
            egui::Sense::hover(),
        );
        for (fraction, text, align) in [
            (0.0, "00", egui::Align2::LEFT_TOP),
            (0.25, "06", egui::Align2::CENTER_TOP),
            (0.5, "12", egui::Align2::CENTER_TOP),
            (0.75, "18", egui::Align2::CENTER_TOP),
            (1.0, "24", egui::Align2::RIGHT_TOP),
        ] {
            let x = ticks.left() + ticks.width() * fraction;
            ui.painter().text(
                egui::pos2(x, ticks.top()),
                align,
                text,
                egui::FontId::proportional(self.text.caption),
                colours.muted,
            );
        }
    }

    /// The two buttons under the tiles: the best-spot session and the details.
    /// The actions of the home screen: the pairing code, the firewall rule,
    /// the big button and the pair row. The big button belongs to the card
    /// of the tile home, so it is left out there; the other buttons of that
    /// path are at least a finger high.
    fn home_actions(
        &mut self,
        ui: &mut egui::Ui,
        colours: &Colours,
        v: &View,
        actions: &mut Vec<Action>,
        primary: bool,
    ) {
        let metrics = theme::metrics();
        let resolved = self.resolved_theme();
        if v.show_code_entry {
            if let Some(hint) = &v.code_hint {
                ui.label(hint);
            }
            ui.add_space(4.0);
            if self.code_boxes(ui) {
                actions.push(Action::SubmitCode);
            }
            if let Some(e) = &self.code_error {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("! ").color(colours.err));
                    ui.label(egui::RichText::new(e).color(colours.fg));
                });
            }
            ui.add_space(8.0);
        }
        if v.offer_rule {
            let height = if primary { 32.0 } else { 44.0 };
            if ui
                .add(
                    egui::Button::new("Allow on this Wi-Fi interface")
                        .min_size(egui::vec2(220.0, height)),
                )
                .clicked()
            {
                actions.push(Action::AllowFirewall);
            }
        }
        if v.polkit_note {
            ui.label(egui::RichText::new(POLKIT_NOTE).weak());
        }
        if primary {
            let label = egui::RichText::new(v.primary.label()).size(self.text.body_large);
            let button = egui::Button::new(label).min_size(egui::vec2(220.0, 40.0));
            if ui.add_enabled(v.primary_enabled, button).clicked() {
                actions.push(Action::Primary(v.primary));
            }
        }
        ui.horizontal(|ui| {
            if v.show_pair_row {
                if ui
                    .add(egui::Button::new("Pair new device").frame(false))
                    .clicked()
                {
                    actions.push(Action::PairNew);
                }
                if ui
                    .add(egui::Button::new("Forget device").frame(false))
                    .clicked()
                {
                    actions.push(Action::Forget);
                }
            }
            if v.show_cancel_entry && ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                actions.push(Action::CancelEntry);
            }
        });
        if state_from(&self.snapshot.state) != ConnectionState::Idle
            && !self.boxes.code().is_empty()
        {
            ui.add_space(metrics.spacing.md);
            resolved.card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("Pairing code")
                            .size(self.text.caption)
                            .color(colours.muted),
                    );
                    ui.add_space(metrics.spacing.xs);
                    ui.label(
                        egui::RichText::new(spaced_code(&self.boxes.code()))
                            .font(egui::FontId::monospace(self.text.display))
                            .color(colours.fg)
                            .strong(),
                    );
                });
            });
        }
    }

    /// The top of every non-home page: Back at the left, then the title.
    /// Returns true when Back was clicked.
    fn page_header(&self, ui: &mut egui::Ui) -> bool {
        let metrics = theme::metrics();
        let mut back = false;
        ui.horizontal(|ui| {
            let button = egui::Button::new(egui::RichText::new("< Back").size(self.text.body))
                .min_size(egui::vec2(84.0, 36.0));
            if ui.add(button).clicked() {
                back = true;
            }
            ui.add_space(metrics.spacing.sm);
            ui.label(
                egui::RichText::new(self.nav.current().title())
                    .font(semibold(self.text.title)),
            );
        });
        back
    }

    /// Draw one page. The home is drawn by the frame itself; this is the body
    /// under the Back header for every other page.
    fn page_body(
        &mut self,
        ui: &mut egui::Ui,
        page: Page,
        colours: &Colours,
        v: &View,
        actions: &mut Vec<Action>,
    ) {
        match page {
            Page::Home => {}
            Page::Details => self.details_body(ui),
            Page::Drops => self.drops_block(ui, colours),
            Page::Log => self.log_body(ui, colours),
            Page::BestSpot => self.best_spot_page(ui, colours),
            Page::Statistics => self.statistics_body(ui, colours),
            Page::Settings => self.settings_body(ui, colours),
            Page::HomeLayout => self.home_layout_block(ui, colours),
            Page::Pairing => self.pairing_body(ui, colours, v, actions),
            // About is a modal, not a page; open_page converts the hint.
            Page::About => {}
            Page::Notices => self.notices_body(ui, colours),
        }
    }

    fn best_spot_page(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        if self.best_spot_on() {
            self.best_spot_block(ui, colours);
        } else {
            ui.label(
                egui::RichText::new("Turn on Best spot under Settings, Experimental.")
                    .color(colours.muted),
            );
        }
        self.page_details_note(ui, colours);
    }

    /// The result line of an action, on the page that ran it. The note is
    /// shared, but each page draws it, so a clear or a failed start is never
    /// invisible after the move off the old Details block.
    fn page_details_note(&self, ui: &mut egui::Ui, colours: &Colours) {
        if let Some(note) = &self.details_note {
            ui.add_space(theme::metrics().spacing.sm);
            ui.label(
                egui::RichText::new(note)
                    .size(self.text.caption)
                    .color(colours.muted),
            );
        }
    }

    fn pairing_body(
        &mut self,
        ui: &mut egui::Ui,
        colours: &Colours,
        v: &View,
        actions: &mut Vec<Action>,
    ) {
        let text = self.text;
        let metrics = theme::metrics();
        let paired = self.snapshot.paired_name.clone();
        if let Some(name) = &paired {
            settings_value_row(
                ui,
                text,
                colours,
                "Paired with",
                name,
                "Stop on Home ends the connection; this page only changes the pairing.",
            );
        } else {
            ui.label(
                egui::RichText::new("Pair this computer with the phone.")
                    .size(text.body)
                    .color(colours.fg),
            );
            ui.label(
                egui::RichText::new("The phone shows a 4-digit code. Enter it here.")
                    .size(text.caption)
                    .color(colours.muted),
            );
        }
        if v.show_code_entry {
            ui.add_space(metrics.spacing.md);
            if let Some(hint) = &v.code_hint {
                ui.label(
                    egui::RichText::new(hint)
                        .size(text.caption)
                        .color(colours.muted),
                );
            }
            ui.add_space(metrics.spacing.xs);
            if self.code_boxes(ui) {
                actions.push(Action::SubmitCode);
            }
            if let Some(e) = &self.code_error {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("! ").color(colours.err));
                    ui.label(egui::RichText::new(e).color(colours.fg));
                });
            }
        }
        if v.offer_rule {
            ui.add_space(metrics.spacing.md);
            if ui
                .add(
                    egui::Button::new("Allow on this Wi-Fi interface")
                        .min_size(egui::vec2(220.0, 44.0)),
                )
                .clicked()
            {
                actions.push(Action::AllowFirewall);
            }
        }
        if v.polkit_note {
            ui.label(egui::RichText::new(POLKIT_NOTE).weak());
        }
        // The primary action is Connect while unpaired (or while a new code
        // is being entered); a paired page has no Disconnect, because Stop
        // lives on Home.
        if paired.is_none() || v.show_code_entry {
            ui.add_space(metrics.spacing.md);
            let label = egui::RichText::new(v.primary.label()).size(text.body_large);
            let button = egui::Button::new(label).min_size(egui::vec2(220.0, 40.0));
            if ui.add_enabled(v.primary_enabled, button).clicked() {
                actions.push(Action::Primary(v.primary));
            }
        }
        if v.show_pair_row {
            ui.add_space(metrics.spacing.md);
            ui.horizontal(|ui| {
                let pair = egui::Button::new(egui::RichText::new("Pair new device").size(text.body))
                    .min_size(egui::vec2(160.0, 40.0));
                if ui.add(pair).clicked() {
                    actions.push(Action::PairNew);
                }
                let forget =
                    egui::Button::new(egui::RichText::new("Forget device").size(text.body))
                        .min_size(egui::vec2(160.0, 40.0));
                if ui.add(forget).clicked() {
                    actions.push(Action::Forget);
                }
            });
        }
        if v.show_cancel_entry {
            ui.add_space(metrics.spacing.sm);
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("Cancel").size(text.body))
                        .min_size(egui::vec2(120.0, 36.0)),
                )
                .clicked()
            {
                actions.push(Action::CancelEntry);
            }
        }
        if state_from(&self.snapshot.state) != ConnectionState::Idle
            && !self.boxes.code().is_empty()
        {
            ui.add_space(metrics.spacing.md);
            let resolved = self.resolved_theme();
            resolved.card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("Pairing code")
                            .size(text.caption)
                            .color(colours.muted),
                    );
                    ui.add_space(metrics.spacing.xs);
                    ui.label(
                        egui::RichText::new(spaced_code(&self.boxes.code()))
                            .font(egui::FontId::monospace(text.display))
                            .color(colours.fg)
                            .strong(),
                    );
                });
            });
        }
    }

    fn notices_body(&self, ui: &mut egui::Ui, colours: &Colours) {
        let text = self.text;
        for (title, body) in THIRD_PARTY_NOTICES {
            ui.label(egui::RichText::new(*title).font(semibold(text.body)));
            ui.label(
                egui::RichText::new(*body)
                    .size(text.caption)
                    .color(colours.muted),
            );
            ui.add_space(theme::metrics().spacing.sm);
        }
    }

    fn details_body(&mut self, ui: &mut egui::Ui) {
        let error = self
            .start_error
            .as_deref()
            .or(self.snapshot.last_error.as_deref())
            .or(self.snapshot_error.as_deref());
        let colours = self.colours();
        let sections = jafnify_linux::details::sections(&self.snapshot, error);
        for (index, section) in sections.iter().enumerate() {
            if index > 0 {
                ui.add_space(theme::metrics().spacing.md);
            }
            detail_section(ui, &colours, self.text, section, ("details", index));
        }
        // The phone's measurements are data, not a setting: they live here in
        // Details (like the phone's main screen), not under Experimental.
        let phone_on = self
            .snapshot
            .flags
            .get("phone_stats")
            .copied()
            .unwrap_or(false);
        let phone_lines = jafnify_linux::flags::phone_stats_lines(
            phone_on,
            self.snapshot.state == "connected",
            self.snapshot.phone_stats.as_ref(),
            self.snapshot.phone_stats_age_secs,
            self.snapshot.phone_stats_unavailable,
            self.link_test_result,
            self.wifi_leg().as_ref(),
        );
        // The heat warning and the good-time notice are two more lines of the
        // phone block; each one has its own switch.
        let heat_on = self
            .snapshot
            .flags
            .get("phone_heat_warning")
            .copied()
            .unwrap_or(false);
        let good_time_on = self
            .snapshot
            .flags
            .get("good_time_notice")
            .copied()
            .unwrap_or(false);
        let phone_lines = jafnify_linux::flags::heat_lines(
            phone_lines,
            heat_on,
            self.snapshot.phone_hot_temp_c_x10,
        );
        let phone_lines = jafnify_linux::flags::good_time_lines(
            phone_lines,
            good_time_on,
            self.snapshot.phone_good_time,
        );
        let block = jafnify_linux::pages::phone_block(&phone_lines);
        if !block.rows.is_empty() || !block.notes.is_empty() {
            ui.add_space(theme::metrics().spacing.md);
            let section = jafnify_linux::details::DetailSection {
                title: jafnify_core::phone_stats::HEADING,
                rows: block
                    .rows
                    .into_iter()
                    .map(|(label, value)| jafnify_linux::details::DetailRow {
                        label,
                        value,
                        warn: false,
                    })
                    .collect(),
            };
            detail_section(ui, &colours, self.text, &section, ("phone", 0));
            for note in block.notes {
                ui.label(
                    egui::RichText::new(note)
                        .size(self.text.caption)
                        .color(colours.muted),
                );
            }
        }
        // The rows that lead to their own page, in the same row language as
        // the settings rows.
        ui.add_space(theme::metrics().spacing.md);
        if self.drops_on()
            && settings_link_row(
                ui,
                self.text,
                &colours,
                "Drops",
                "The connection faults of the last day.",
            )
        {
            self.open_page(Page::Drops);
        }
        if settings_link_row(
            ui,
            self.text,
            &colours,
            "Connection log",
            "The redacted log of this computer.",
        ) {
            self.open_page(Page::Log);
        }
        if let Some(note) = &self.details_note {
            ui.add_space(theme::metrics().spacing.sm);
            ui.label(egui::RichText::new(note).color(colours.muted));
        }
    }

    /// True while the best-spot switch is on.
    fn best_spot_on(&self) -> bool {
        self.snapshot
            .flags
            .get("best_spot")
            .copied()
            .unwrap_or(false)
    }

    /// The best-spot block: what the session does, the buttons that run it
    /// and the lines it produced. The daemon owns the session.
    fn best_spot_block(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let text = self.text;
        let minutes = jafnify_core::best_spot::SESSION_SECS / 60;
        ui.label(
            egui::RichText::new(format!(
                "Measures the phone for {minutes} minutes. Move it to another place, tap This spot, and the marks are compared at the end."
            ))
            .size(text.body)
            .color(colours.fg),
        );
        ui.add_space(theme::metrics().spacing.md);
        ui.horizontal(|ui| {
            let start = egui::Button::new(egui::RichText::new("Start").size(text.body))
                .min_size(egui::vec2(120.0, 40.0));
            if ui.add(start).clicked() {
                self.best_spot_command(Request::BestSpotStart);
            }
            let mark = egui::Button::new(egui::RichText::new("This spot").size(text.body))
                .min_size(egui::vec2(120.0, 40.0));
            if ui
                .add_enabled(self.snapshot.best_spot_running, mark)
                .clicked()
            {
                self.best_spot_command(Request::BestSpotMark);
            }
            let stop = egui::Button::new(egui::RichText::new("Stop").size(text.body))
                .min_size(egui::vec2(120.0, 40.0));
            if ui
                .add_enabled(self.snapshot.best_spot_showing(), stop)
                .clicked()
            {
                self.best_spot_command(Request::BestSpotStop);
            }
        });
        // Say why a button is disabled, in one muted line, instead of leaving
        // the user to guess.
        if !self.snapshot.best_spot_running {
            ui.label(
                egui::RichText::new("This spot is available while the session runs.")
                    .size(text.caption)
                    .color(colours.muted),
            );
        }
        if !self.snapshot.best_spot_showing() {
            ui.label(
                egui::RichText::new("Stop is available after a spot is marked.")
                    .size(text.caption)
                    .color(colours.muted),
            );
        }
        ui.add_space(theme::metrics().spacing.sm);
        for line in &self.snapshot.best_spot_lines {
            ui.label(egui::RichText::new(line.clone()).color(colours.muted));
        }
    }

    /// Send one best-spot command to the daemon and say what went wrong.
    fn best_spot_command(&mut self, request: Request) {
        if let Err(e) = self.send(request) {
            self.details_note = Some(format!("Could not run the best-spot session: {e}"));
        }
    }

    /// True while the good-hours switch is on.
    fn good_hours_on(&self) -> bool {
        self.snapshot
            .flags
            .get("good_hours")
            .copied()
            .unwrap_or(false)
    }

    /// The good-hours block: what the learning knows and the button that
    /// clears it. The heading and the learning line belong to the page and
    /// the strip above, so they are not repeated here. The daemon owns the
    /// file.
    fn good_hours_block(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let lines =
            jafnify_linux::pages::good_hours_detail_lines(&self.snapshot.good_hours_lines);
        ui.add_space(theme::metrics().spacing.sm);
        for line in lines {
            ui.label(egui::RichText::new(line).color(colours.muted));
        }
        let clear = egui::Button::new(
            egui::RichText::new("Clear good hours").size(self.text.body),
        )
        .min_size(egui::vec2(84.0, 36.0));
        if ui.add(clear).clicked() {
            self.confirm = Some(Confirm::ClearGoodHours);
        }
    }

    /// The drop history of the last day, in the settings row language: the
    /// summary row, one row per drop and the button that clears the file. The
    /// daemon owns the file.
    fn drops_block(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let text = self.text;
        match (&self.drops, &self.drops_error) {
            (Some(drops), _) => {
                let (title, value, help) = jafnify_linux::pages::drop_summary_row(&drops.summary);
                settings_value_row(ui, text, colours, &title, &value, &help);
                for row in drops.rows.iter().take(DROP_ROWS_SHOWN) {
                    let parts = jafnify_linux::pages::drop_parts(row);
                    settings_value_row(
                        ui,
                        text,
                        colours,
                        &parts.time,
                        &parts.duration,
                        &parts.cause,
                    );
                }
            }
            (None, Some(e)) => settings_value_row(
                ui,
                text,
                colours,
                "Drop history",
                "unavailable",
                &format!("The drop history is not available: {e}"),
            ),
            (None, None) => settings_value_row(
                ui,
                text,
                colours,
                "Drop history",
                "reading",
                "Reading the drop history...",
            ),
        }
        ui.add_space(theme::metrics().spacing.md);
        let clear = egui::Button::new(egui::RichText::new("Clear drop history").size(text.body))
            .min_size(egui::vec2(180.0, 40.0));
        if ui.add(clear).clicked() {
            self.confirm = Some(Confirm::ClearDrops);
        }
        self.page_details_note(ui, colours);
    }

    /// True while the drop history switch is on.
    fn drops_on(&self) -> bool {
        self.snapshot
            .flags
            .get("drop_history")
            .copied()
            .unwrap_or(false)
    }

    /// The passive PC leg of the last phone requests.
    fn wifi_leg(&self) -> Option<WifiLeg> {
        match (
            self.snapshot.wifi_ms,
            self.snapshot.wifi_loss_pct,
            self.snapshot.wifi_n,
        ) {
            (Some(median_ms), Some(loss_pct), Some(samples)) => {
                Some(WifiLeg::new(median_ms, loss_pct, samples as usize))
            }
            _ => None,
        }
    }

    /// Read the drop history from the daemon, at most every few seconds.
    fn poll_drops(&mut self) {
        if !self.drops_on() {
            self.drops = None;
            self.drops_error = None;
            self.drops_read_at = None;
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.drops_read_at {
            if now.duration_since(last) < DROPS_POLL_INTERVAL {
                return;
            }
        }
        self.drops_read_at = Some(now);
        self.read_drops();
    }

    /// Ask the daemon for the drop history.
    fn read_drops(&mut self) {
        match control::request(&Request::GetDrops) {
            Ok(Response::Drops { summary, rows }) => {
                self.drops = Some(DropsView { summary, rows });
                self.drops_error = None;
            }
            Ok(_) => {}
            Err(e) => {
                self.drops = None;
                self.drops_error = Some(e.to_string());
            }
        }
    }

    /// The Settings page: flat rows in groups, no collapsing header, one
    /// action per row. The last group carries the About links and Save.
    fn settings_body(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        use jafnify_core::flags::Bucket;
        let text = self.text;
        let metrics = theme::metrics();

        settings_heading(ui, text, colours, "General");
        let mut autostart = self.autostart_tmp;
        if settings_row(
            ui,
            text,
            colours,
            "Auto start",
            "Start Jafnify when you log in.",
            |ui| ui.checkbox(&mut autostart, ""),
        )
        .changed()
        {
            self.autostart_tmp = autostart;
        }
        let mut tray = self.tray_tmp;
        if settings_row(
            ui,
            text,
            colours,
            "Tray icon",
            "Show the tray icon only while the window is closed.",
            |ui| ui.checkbox(&mut tray, ""),
        )
        .changed()
        {
            self.tray_tmp = tray;
        }
        let mut name = self.nickname_tmp.clone();
        if settings_row(
            ui,
            text,
            colours,
            "This device name",
            "Shown only on this computer, never to the phone.",
            |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .desired_width(240.0)
                        .hint_text("Name for this computer"),
                )
            },
        )
        .changed()
        {
            self.nickname_tmp = name;
        }
        let chosen = settings_row(
            ui,
            text,
            colours,
            "Theme",
            "Match the desktop, or always use dark or light.",
            |ui| {
                let mut chosen = None;
                for (mode, label) in [
                    (ThemeMode::System, "System"),
                    (ThemeMode::Dark, "Dark"),
                    (ThemeMode::Light, "Light"),
                ] {
                    if choice(ui, colours, self.theme_tmp == mode, label).clicked() {
                        chosen = Some(mode);
                    }
                }
                chosen
            },
        );
        if let Some(mode) = chosen {
            self.choose_theme(mode);
        }

        ui.add_space(metrics.spacing.md);
        settings_heading(ui, text, colours, "Connection");
        let mut kill = self.kill_tmp;
        if settings_row(
            ui,
            text,
            colours,
            "Kill switch",
            "Block traffic outside the tunnel, including while it is down. Applies at the next connect.",
            |ui| ui.checkbox(&mut kill, ""),
        )
        .changed()
        {
            self.kill_tmp = kill;
        }
        let role = settings_row(
            ui,
            text,
            colours,
            "Role",
            "Join a phone's connection, or share this computer's internet.",
            |ui| {
                let mut chosen = None;
                if choice(ui, colours, self.role_tmp == "joiner", "Join peer").clicked() {
                    chosen = Some("joiner");
                }
                if choice(ui, colours, self.role_tmp == "sharer", "Share internet").clicked() {
                    chosen = Some("sharer");
                }
                chosen
            },
        );
        if let Some(role) = role {
            self.role_tmp = role.to_string();
        }
        self.flag_rows(ui, colours, Bucket::Settings);
        self.flag_rows(ui, colours, Bucket::Advanced);
        // Pairing is its own page: the code entry, pair new, forget and the
        // firewall offer live there, so pairing stays reachable from Settings
        // on both a fresh and a paired install.
        if settings_link_row(
            ui,
            text,
            colours,
            "Pairing",
            "Pair a new phone or forget the paired one.",
        ) {
            self.open_page(Page::Pairing);
        }

        ui.add_space(metrics.spacing.md);
        settings_heading(ui, text, colours, "Home");
        if settings_link_row(
            ui,
            text,
            colours,
            "Home layout",
            "Choose the tiles on the home screen.",
        ) {
            self.open_page(Page::HomeLayout);
        }

        ui.add_space(metrics.spacing.md);
        settings_heading(ui, text, colours, "Experimental");
        if let Some(section) = jafnify_linux::flags::sections(&self.snapshot.flags)
            .into_iter()
            .find(|section| section.bucket == Bucket::Experimental)
        {
            if let Some(note) = section.note {
                ui.label(
                    egui::RichText::new(note)
                        .size(text.caption)
                        .color(colours.muted),
                );
            }
        }
        self.flag_rows(ui, colours, Bucket::Experimental);
        let link_pending = self.link_test_rx.is_some();
        let start_link = settings_row(
            ui,
            text,
            colours,
            "Link test",
            "Measure one round trip to the phone.",
            |ui| {
                let run = egui::Button::new(egui::RichText::new("Run").size(text.body))
                    .min_size(egui::vec2(84.0, 36.0));
                ui.add_enabled(!link_pending, run)
            },
        )
        .clicked();
        if start_link {
            self.start_link_test();
        }
        if let Some(line) = &self.link_test_line {
            ui.label(egui::RichText::new(line).size(text.caption).color(colours.muted));
        }

        ui.add_space(metrics.spacing.md);
        settings_heading(ui, text, colours, "About");
        if settings_link_row(
            ui,
            text,
            colours,
            "About Jafnify",
            "Version, credits and diagnostics.",
        ) {
            self.about_open = true;
        }
        if settings_link_row(
            ui,
            text,
            colours,
            "Third-party notices",
            "The licences of the software Jafnify uses.",
        ) {
            self.open_page(Page::Notices);
        }

        ui.add_space(metrics.spacing.md);
        let save = egui::Button::new(egui::RichText::new("Save settings").size(text.body))
            .min_size(egui::vec2(160.0, 40.0));
        if ui.add(save).clicked() {
            self.save_settings();
        }
        if let Some(note) = &self.settings_note {
            ui.label(egui::RichText::new(note).size(text.caption).color(colours.muted));
        }
    }

    /// The home layout choices in the settings row language: the grid, the
    /// wide slot of a 7 grid and the tile of every slot, the control on the
    /// right. Any change is sent at once; the daemon validates it with the
    /// core and writes the file, so no restart is needed.
    fn home_layout_block(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        use jafnify_core::home_layout::{Grid, Layout, Platform, WideAt};
        let text = self.text;
        let scopes = self.snapshot.tile_scopes.clone();
        let has_session = self.snapshot.current_session.started_at > 0;
        let caption = |id: &str| {
            let scope = TileScope::for_tile(id, scopes.get(id).map(String::as_str));
            let scope = if scope == TileScope::Session && !has_session {
                TileScope::default_for(id)
            } else {
                scope
            };
            jafnify_linux::pages::home_slot_caption(id, scope.word())
        };
        let mut layout = Layout::parse(&self.snapshot.home_layout)
            .unwrap_or_else(jafnify_linux::home_layout::default_layout)
            .normalize(Platform::Linux);
        let mut changed = false;
        settings_control_row(ui, text, colours, "Tiles", |ui| {
            // The control sits against the right edge (right-to-left), so the
            // choices are added from 8 down to 6 to read 6 7 8 on the page.
            for grid in Grid::ALL.into_iter().rev() {
                if choice(ui, colours, layout.grid == grid, grid.key()).clicked() {
                    layout.grid = grid;
                    changed = true;
                }
            }
        });
        if layout.grid == Grid::Seven {
            settings_control_row(ui, text, colours, "Wide tile", |ui| {
                for (wide, label) in [(WideAt::Bottom, "Bottom"), (WideAt::Top, "Top")] {
                    if choice(ui, colours, layout.wide == wide, label).clicked() {
                        layout.wide = wide;
                        changed = true;
                    }
                }
            });
        }
        for (index, current) in layout.slots.clone().iter().enumerate() {
            let mut chosen = current.clone();
            settings_control_row(ui, text, colours, &format!("Slot {}", index + 1), |ui| {
                let selected = caption(&chosen);
                egui::ComboBox::from_id_salt(("home-layout-slot", index))
                    .width(180.0)
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (id, _) in jafnify_core::home_layout::catalog(Platform::Linux) {
                            if ui.selectable_label(chosen == *id, caption(id)).clicked() {
                                chosen = (*id).to_string();
                            }
                        }
                    });
            });
            if chosen != *current {
                layout.slots[index] = chosen;
                changed = true;
            }
        }
        if changed {
            let text = layout.normalize(Platform::Linux).to_text();
            self.settings_note = Some(match self.send(Request::SetHomeLayout { layout: text }) {
                Ok(_) => "Home layout saved".to_string(),
                Err(e) => e,
            });
        }
        if let Some(note) = &self.settings_note {
            ui.label(
                egui::RichText::new(note)
                    .size(self.text.caption)
                    .color(colours.muted),
            );
        }
    }

    /// The Log page: the redacted connection log, newest first, with the
    /// copy/save actions on top. The page opens on the newest line. The page
    /// scrolls; the log is data.
    fn log_body(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let text = self.text;
        ui.horizontal(|ui| {
            if ui.button("Copy log").clicked() {
                let text = export::redact_lines(&self.snapshot.connection_log);
                let count = self.snapshot.connection_log.len();
                ui.ctx().copy_text(text);
                self.log_note = Some(format!(
                    "Copied {count} lines (redacted: public addresses and peer identifiers)."
                ));
            }
            if ui.button("Copy diagnostics").clicked() {
                self.log_note = Some(self.copy_diagnostics(ui.ctx()));
            }
            if ui.button("Save log").clicked() {
                self.log_note = Some(self.save_log());
            }
        });
        if let Some(note) = &self.log_note {
            ui.label(egui::RichText::new(note).size(text.caption).color(colours.muted));
        }
        ui.add_space(theme::metrics().spacing.sm);
        if self.snapshot.connection_log.is_empty() {
            ui.label(egui::RichText::new("No log lines yet").color(colours.muted));
        }
        for line in self.snapshot.connection_log.iter().rev() {
            ui.monospace(line);
        }
    }

    /// The Statistics page: the laptop's own usage, the usual hours strip,
    /// the good-hours learning and the session history. It may scroll.
    fn statistics_body(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        let text = self.text;
        let metrics = theme::metrics();
        settings_heading(ui, text, colours, "Usage statistics");
        if let Some(usage) = &self.snapshot.usage {
            for (title, lines) in [
                ("Today", &usage.today),
                ("7 days", &usage.week),
                ("30 days", &usage.month),
            ] {
                ui.label(egui::RichText::new(title).font(semibold(text.body)));
                for line in lines {
                    if let Some(line) = jafnify_linux::pages::usage_display_line(line) {
                        ui.label(egui::RichText::new(line).size(text.body).color(colours.fg));
                    }
                }
            }
        } else {
            ui.label(
                egui::RichText::new("Usage statistics: not enough data yet")
                    .size(text.caption)
                    .color(colours.muted),
            );
        }
        if let Some(phone) = &self.snapshot.phone_stats {
            let line = jafnify_core::budget::display_for(phone.used_bytes, phone.allow_bytes);
            ui.add_space(metrics.spacing.sm);
            ui.label(
                egui::RichText::new("Monthly allowance")
                    .size(text.body)
                    .color(colours.fg),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{line} The plan the phone reports; it is the phone's own budget."
                    ))
                    .size(text.caption)
                    .color(colours.muted),
                )
                .wrap(),
            );
            ui.add_space(metrics.spacing.xs);
            ui.separator();
        }

        // One heading for the busy hours: the strip's own title is left out,
        // and the learning line is drawn once, by the strip.
        if self.snapshot.home.as_ref().and_then(|home| home.hours.as_ref()).is_some()
            || self.good_hours_on()
        {
            ui.add_space(metrics.spacing.md);
            settings_heading(ui, text, colours, "Busy hours");
            if let Some(strip) =
                self.snapshot.home.as_ref().and_then(|home| home.hours.clone())
            {
                self.hours_strip(ui, colours, &strip, false);
            }
            if self.good_hours_on() {
                self.good_hours_block(ui, colours);
            }
        }
        ui.add_space(metrics.spacing.md);
        self.session_history_body(ui, colours);
        self.page_details_note(ui, colours);
    }

    fn session_history_body(&mut self, ui: &mut egui::Ui, colours: &Colours) {
        settings_heading(ui, self.text, colours, "Session history");
        let history = self.snapshot.session_history.clone();
        if history.is_empty() {
            ui.label(
                egui::RichText::new("No completed sessions yet")
                    .size(self.text.caption)
                    .color(colours.muted),
            );
        }
        for record in &history {
            egui::CollapsingHeader::new(jafnify_linux::pages::history_headline(record))
                .id_salt(("history", record.id.clone()))
            .show(ui, |ui| {
                let colours = self.colours();
                let section = jafnify_linux::details::DetailSection {
                    title: "",
                    rows: jafnify_linux::details::history_rows(record),
                };
                detail_section(ui, &colours, self.text, &section, ("history-rows", 0));
                if !record.reconnects.is_empty() {
                    for (index, reconnect) in record.reconnects.iter().enumerate() {
                        egui::CollapsingHeader::new(format!(
                            "Reconnect {} ({} ms)",
                            index + 1,
                            reconnect.duration_ms
                        ))
                        .id_salt(("reconnect", record.id.clone(), index))
                        .show(ui, |ui| {
                            ui.label(format!(
                                "Disconnected: {}",
                                format_timestamp_or_none(reconnect.disconnected_at)
                            ));
                            ui.label(format!(
                                "Detected: {}",
                                format_timestamp_or_none(reconnect.detected_at)
                            ));
                            let connected = if reconnect.connected_at == 0 {
                                "not reconnected".to_string()
                            } else {
                                format_timestamp_or_none(reconnect.connected_at)
                            };
                            ui.label(format!("Connected: {connected}"));
                            ui.label(format!(
                                "Duration: {}",
                                format_elapsed(reconnect.duration_ms / 1_000)
                            ));
                            ui.label(format!("Reason: {}", reconnect.reason));
                        });
                    }
                }
            });
        }
    }

    /// Ask the daemon for the redacted diagnostics and copy them. The daemon
    /// owns the journal and the application log, so the window never reads
    /// those files itself.
    fn copy_diagnostics(&mut self, ctx: &egui::Context) -> String {
        match control::request(&Request::Diagnostics) {
            Ok(Response::Diagnostics { text }) => {
                ctx.copy_text(text);
                "Copied diagnostics (public addresses and peer identifiers replaced).".to_string()
            }
            Ok(Response::Err { message }) => format!("Could not build the diagnostics: {message}"),
            Ok(_) => "The daemon did not answer with the diagnostics".to_string(),
            Err(e) => format!("Could not build the diagnostics: {e}"),
        }
    }

    /// Write the redacted log to a 0600 file in Downloads. Returns the note
    /// shown under the buttons.
    fn save_log(&self) -> String {
        let Some(dir) =
            dirs::download_dir().or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        else {
            return "Could not find a Downloads folder".to_string();
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return format!("Could not create {}: {e}", dir.display());
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        let text = self.snapshot.connection_log.join("\n");
        match connection_log::save_log_at(&dir, &text, now_ms) {
            Ok(path) => format!("Saved log to {}", path.display()),
            Err(e) => format!("Could not save the log: {e}"),
        }
    }

    fn persist_view_state(&mut self) {
        self.details_note = match self.send(Request::SetViewState {
            details_expanded: self.snapshot.details_expanded,
            advanced_expanded: self.snapshot.advanced_expanded,
        }) {
            Ok(_) => None,
            Err(e) => Some(format!("Could not save the view state: {e}")),
        };
    }

    /// A theme click applies on the next frame and saves right away. It sends
    /// a theme-only request so autostart and kill-switch work cannot run.
    fn choose_theme(&mut self, mode: ThemeMode) {
        self.theme_tmp = mode;
        self.snapshot.theme = mode.as_str().to_string();
        let request = Request::SetTheme {
            theme: mode.as_str().to_string(),
        };
        self.settings_note = Some(match self.send(request) {
            Ok(_) => "Theme saved".to_string(),
            Err(e) => e,
        });
    }

    /// One settings row per rendered flag of a bucket, as a switch on the
    /// right of the row. A bucket with no rendered row adds no widget.
    fn flag_rows(&mut self, ui: &mut egui::Ui, colours: &Colours, bucket: Bucket) {
        let text = self.text;
        for section in jafnify_linux::flags::sections(&self.snapshot.flags) {
            if section.bucket != bucket {
                continue;
            }
            for row in section.rows {
                let mut value = row.value;
                let response = settings_row(ui, text, colours, row.label, row.help, |ui| {
                    ui.checkbox(&mut value, "")
                });
                if response.changed() {
                    self.save_flag(row.id, value);
                }
            }
        }
    }

    fn poll_link_test(&mut self) {
        let received = match &self.link_test_rx {
            Some(receiver) => receiver.try_recv(),
            None => return,
        };
        match received {
            Ok((line, result)) => {
                self.link_test_line = Some(line);
                self.link_test_result = result;
                self.link_test_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.link_test_line =
                    Some(jafnify_core::link_test::LinkTestResult::from_samples(&[]).line());
                self.link_test_result = None;
                self.link_test_rx = None;
            }
        }
    }

    fn start_link_test(&mut self) {
        if self.link_test_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (line, result) =
                match control::request_timeout(&Request::LinkTest, Duration::from_secs(15)) {
                    Ok(Response::LinkTest {
                        line,
                        sent,
                        avg_ms,
                        loss_pct,
                    }) => {
                        let result = (sent > 0).then_some(LinkLeg {
                            avg_ms: avg_ms.unwrap_or(0),
                            loss_pct: u64::from(loss_pct),
                        });
                        (line, result)
                    }
                    Ok(Response::Err { message }) => (message, None),
                    Ok(_) => (jafnify_core::link_test::connect_first_line(), None),
                    Err(e) => (e.to_string(), None),
                };
            let _ = tx.send((line, result));
        });
        self.link_test_rx = Some(rx);
        self.link_test_line = None;
        self.link_test_result = None;
    }

    /// A flag switch applies at once, like the switches on the phone: the
    /// Experimental group has no Save button of its own. A narrow SetFlag
    /// leaves unattended settings, autostart and the kill switch alone.
    fn save_flag(&mut self, id: &str, value: bool) {
        let request = Request::SetFlag {
            id: id.to_string(),
            value,
        };
        match self.send(request) {
            Ok(_) => {
                self.snapshot.flags.insert(id.to_string(), value);
            }
            Err(e) => self.settings_note = Some(e),
        }
    }

    fn save_settings(&mut self) {
        let request = Request::SetSettings {
            role: self.role_tmp.clone(),
            autostart: self.autostart_tmp,
            kill_switch: self.kill_tmp,
            tray_only_when_closed: self.tray_tmp,
            theme: Some(self.theme_tmp.as_str().to_string()),
            flags: BTreeMap::new(),
            nickname: Some(self.nickname_tmp.clone()),
        };
        let result = self.send(request);
        self.settings_loaded = false;
        self.settings_note = Some(match result {
            Ok(_) => "Settings saved".to_string(),
            Err(e) => e,
        });
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(confirm) = self.confirm else {
            return;
        };
        let name = self.snapshot.paired_name.clone().unwrap_or_default();
        let iface = self.snapshot.firewall_iface.clone();
        let scope = match iface.as_deref() {
            Some(name) => format!("the Wi-Fi interface ({name})"),
            None => "the Wi-Fi interface".to_string(),
        };
        let (title, message, yes) = match confirm {
            Confirm::AllowFirewall => (
                "Allow on this Wi-Fi interface?",
                format!(
                    "Jafnify will ask for your password and add ONE firewall rule: allow UDP \
                     port 9799 to 239.255.42.99 on {scope}. Nothing else changes and the \
                     firewall stays on."
                ),
                "Add rule",
            ),
            Confirm::Forget => (
                "Forget device?",
                "This device will be forgotten. The other device keeps trying to connect until it forgets this one too.".to_string(),
                "Forget",
            ),
            Confirm::PairNew => (
                "Pair new device?",
                format!("The new pairing replaces {name}."),
                "Pair new device",
            ),
            Confirm::Terms => (
                "Before you start",
                jafnify_core::terms::DISCLAIMER.to_string(),
                "I understand",
            ),
            Confirm::Stop => (
                "Stop the connection?",
                "The tunnel will close. You can connect again from the home page.".to_string(),
                "Stop",
            ),
            Confirm::ClearDrops => (
                "Clear the drop history?",
                "The drops kept for the last day will be deleted.".to_string(),
                "Clear",
            ),
            Confirm::ClearGoodHours => (
                "Clear the good hours?",
                "What Jafnify learned about the busy hours will be deleted.".to_string(),
                "Clear",
            ),
        };
        let text = self.text;
        let colours = self.colours();
        let mut answer = None;
        let response =
            egui::Modal::new(egui::Id::unique("jafnify-confirm")).show(ctx, |ui| {
            ui.set_max_width(360.0);
            ui.label(egui::RichText::new(title).font(semibold(text.title)));
            ui.add_space(theme::metrics().spacing.sm);
            ui.label(egui::RichText::new(message).size(text.body).color(colours.fg));
            ui.add_space(theme::metrics().spacing.md);
            ui.horizontal(|ui| {
                let yes_button = egui::Button::new(egui::RichText::new(yes).size(text.body))
                    .min_size(egui::vec2(96.0, 36.0));
                if ui.add(yes_button).clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        if response.backdrop_response.clicked() {
            answer = Some(false);
        }
        if let Some(yes) = answer {
            self.confirm = None;
            if yes {
                self.answer(confirm);
            }
        }
    }

    /// The About modal over a dimmed page. It is one short decision plus two
    /// links: the notices page and the diagnostic export.
    fn about_modal(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let text = self.text;
        let colours = self.colours();
        let version = env!("CARGO_PKG_VERSION");
        let mut close = false;
        let mut notices = false;
        let mut diagnostics = false;
        let response =
            egui::Modal::new(egui::Id::unique("jafnify-about-modal")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.label(
                egui::RichText::new(jafnify_linux::flavor::DISPLAY)
                    .font(semibold(text.title)),
            );
            ui.label(
                egui::RichText::new(format!("Version {version}"))
                    .size(text.caption)
                    .color(colours.muted),
            );
            ui.label(
                egui::RichText::new("Silicon Stencil")
                    .size(text.body)
                    .color(colours.fg),
            );
            ui.add_space(theme::metrics().spacing.md);
            if ui.button("Third-party notices").clicked() {
                notices = true;
            }
            if ui.button("Report a problem (copy diagnostics)").clicked() {
                diagnostics = true;
            }
            ui.add_space(theme::metrics().spacing.md);
            ui.label(
                egui::RichText::new(jafnify_core::terms::DISCLAIMER)
                    .size(text.caption)
                    .color(colours.muted),
            );
            ui.add_space(theme::metrics().spacing.md);
            if ui.button("Close").clicked() {
                close = true;
            }
        });
        if response.backdrop_response.clicked() {
            close = true;
        }
        if close {
            self.about_open = false;
        }
        if notices {
            self.about_open = false;
            self.open_page(Page::Notices);
        }
        if diagnostics {
            self.log_note = Some(self.copy_diagnostics(ctx));
        }
    }

    /// Remember the window box for the next open. A missing size (the window
    /// never drew) or an out-of-range value is dropped instead of saved.
    /// The current window box, or None before the window has a size.
    fn current_geometry(ctx: &egui::Context) -> Option<WindowGeometry> {
        // Wayland never reports a window position, so inner_rect stays None
        // there; the size comes from the drawable area, which is always known.
        let (inner, outer) = ctx.input(|i| (i.content_rect(), i.viewport().outer_rect));
        if !inner.is_positive() {
            return None;
        }
        let (width, height, position) = scale::native_geometry(
            inner.width(),
            inner.height(),
            outer.map(|rect| (rect.min.x, rect.min.y)),
            ctx.zoom_factor(),
        );
        WindowGeometry {
            width: width.round().max(0.0) as u32,
            height: height.round().max(0.0) as u32,
            x: position.map(|(x, _)| x),
            y: position.map(|(_, y)| y),
        }
        .sanitized()
    }

    fn send_geometry(&mut self, window: WindowGeometry) {
        if control::request(&Request::SetWindowGeometry { window }).is_ok() {
            self.geometry_saved = Some(window);
        }
        self.geometry_pending = None;
    }

    /// Save the window box once it has stopped changing. Saving on resize,
    /// not only on close, keeps it even when the close event never reaches
    /// the app (some compositors end the window without a final frame).
    fn track_geometry(&mut self, ctx: &egui::Context) {
        let Some(now_geometry) = Self::current_geometry(ctx) else {
            return;
        };
        if self.geometry_saved.is_none() {
            // First frame: adopt what the window opened with, do not write.
            self.geometry_saved = Some(now_geometry);
            return;
        }
        if self.geometry_saved == Some(now_geometry) {
            self.geometry_pending = None;
            return;
        }
        match self.geometry_pending {
            Some((pending, since)) if pending == now_geometry => {
                if since.elapsed() >= GEOMETRY_SETTLE {
                    self.send_geometry(now_geometry);
                } else {
                    ctx.request_repaint_after(GEOMETRY_SETTLE);
                }
            }
            _ => {
                self.geometry_pending = Some((now_geometry, Instant::now()));
                ctx.request_repaint_after(GEOMETRY_SETTLE);
            }
        }
    }

    fn save_geometry(&mut self, ctx: &egui::Context) {
        if let Some(window) = Self::current_geometry(ctx) {
            if self.geometry_saved != Some(window) {
                self.send_geometry(window);
            }
        }
    }

    fn send_closed(&mut self) {
        if !self.closed_sent {
            let _ = control::request(&Request::WindowClosed);
            self.closed_sent = true;
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.send_closed();
    }
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let native = {
            let native = ctx.native_pixels_per_point().unwrap_or(1.0);
            if native >= 0.5 { native } else { 1.0 }
        };
        let physical = ctx.viewport_rect().size() * ctx.pixels_per_point();
        if !self.fonts_loaded {
            install_fonts(&ctx);
            self.fonts_loaded = true;
            self.last_text = None;
        }
        // The window is fixed. One start-up fit is allowed when the monitor's
        // work area is smaller than the design box; it is applied once and
        // never changed again.
        if !self.startup_zoom_applied {
            if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
                let zoom = scale::startup_zoom([monitor.x, monitor.y], native);
                self.applied_zoom = zoom;
                if zoom < scale::ZOOM - 0.001 {
                    // A zoom factor alone does not resize the native window;
                    // shrink the fixed box to the work area as well.
                    let scaled = egui::vec2(
                        scale::DESIGN_SIZE[0] * zoom,
                        scale::DESIGN_SIZE[1] * zoom,
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(scaled));
                    ctx.send_viewport_cmd(egui::ViewportCommand::MaxInnerSize(scaled));
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(scaled));
                    ctx.request_repaint();
                }
                if (ctx.zoom_factor() - zoom).abs() > 0.001 {
                    ctx.set_zoom_factor(zoom);
                    ctx.request_repaint();
                }
                self.startup_zoom_applied = true;
            }
        }
        if let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) {
            if let Some(position) = self.pending_position.take() {
                let window = [
                    scale::DESIGN_SIZE[0] * self.applied_zoom,
                    scale::DESIGN_SIZE[1] * self.applied_zoom,
                ];
                let pos = scale::clamp_position(
                    [position.x, position.y],
                    window,
                    [monitor.x, monitor.y],
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                    pos[0], pos[1],
                )));
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_geometry(&ctx);
            self.send_closed();
            return;
        }
        self.track_geometry(&ctx);
        self.poll();
        self.poll_page_hint();
        // The first snapshot after a restart may still carry the old
        // Details-open flag; the probe gate must follow the page on screen.
        self.sync_view_state();
        self.poll_drops();
        self.poll_link_test();
        // Esc closes the top modal first, then goes back one page.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.about_open {
                self.about_open = false;
            } else if self.confirm.is_some() {
                self.confirm = None;
            } else if !self.nav.is_home() {
                self.go_back();
            }
        }
        let resolved = match self.cached_theme {
            Some((mode, theme)) if mode == self.theme_tmp => theme,
            _ => {
                let theme =
                    theme::resolve(self.theme_tmp, self.portal_color_scheme, &self.kdeglobals);
                self.cached_theme = Some((self.theme_tmp, theme));
                theme
            }
        };
        if self.last_applied_theme != Some(resolved) {
            theme::apply(&ctx, resolved);
            self.last_applied_theme = Some(resolved);
            // The theme resets the text styles; publish the floored sizes
            // again on this frame.
            self.last_text = None;
        }
        if self.daemon_gone {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !self.got_snapshot {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        // The style and the width use this frame's pixels-per-point; the new
        // zoom applies on the next pass, and the repaint above gets there at
        // once. Using the requested zoom here would draw one wrong frame.
        self.apply_text(&ctx, ctx.pixels_per_point());
        let v = view(&self.input());
        let colours = self.colours();
        if v.show_code_entry && !self.entry_shown {
            self.focus_first = true;
        }
        self.entry_shown = v.show_code_entry;

        let mut actions = Vec::new();
        self.ensure_brand_texture(&ctx);
        let page = self.nav.current();
        // A non-floating scroll bar allocates its own width, so a page's
        // right-hand control is never under it.
        ctx.all_styles_mut(|style| {
            style.spacing.scroll.floating = false;
            style.spacing.scroll.bar_width = 10.0;
            style.spacing.scroll.bar_inner_margin = 2.0;
            style.spacing.scroll.bar_outer_margin = 2.0;
        });
        egui::CentralPanel::default().show(ui, |ui| {
            if page == Page::Home {
                // The home is one fixed page: no scroll area, so nothing on
                // it can be reached only by scrolling. The width is the
                // panel's interior, so the right margin is never overfilled.
                let width = scale::design_width(ui.available_width(), physical.x, native);
                ui.set_width(width);
                if let Some(texture) = self.brand_tex.clone() {
                    if self.brand_header(ui, &texture) {
                        self.about_open = true;
                    }
                    ui.add_space(theme::metrics().spacing.md);
                }
                if self.tile_home_on() {
                    self.tile_home(ui, &colours, &v, &mut actions);
                } else {
                    self.card(ui, &v);
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        self.home_actions(ui, &colours, &v, &mut actions, true);
                    });
                    self.tile_buttons(ui);
                    ui.add_space(theme::metrics().spacing.sm);
                }
            } else {
                // The Back header stays put; only the page body scrolls, and
                // only when its content truly cannot fit. A non-floating bar
                // takes its own width, so it never covers a right-hand control.
                let width = scale::design_width(ui.available_width(), physical.x, native);
                ui.set_width(width);
                if self.page_header(ui) {
                    self.go_back();
                } else {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .id_salt(("page-scroll", page.key()))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add_space(theme::metrics().spacing.sm);
                            self.page_body(ui, page, &colours, &v, &mut actions);
                        });
                }
            }
        });
        self.confirm_window(&ctx);
        self.about_modal(&ctx);
        for action in actions {
            self.handle(action);
        }

        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

/// The colour a tone gives to a word or a number. The amber of the `wait`
/// tone is the text-safe one: it has to clear 4.5:1 on the surface.
fn tone_colour(colours: &Colours, tone: Tone) -> egui::Color32 {
    match tone {
        Tone::Ok => colours.ok,
        Tone::Wait => colours.wait_text,
        Tone::Err => colours.err,
        Tone::Muted => colours.fg,
    }
}

/// The colour of a mark in an hour strip. A mark is not text, so the bright
/// amber is the one to use.
fn mark_colour(colours: &Colours, mark: CellMark) -> egui::Color32 {
    match mark {
        CellMark::Quick => colours.ok,
        CellMark::Busy => colours.wait,
        CellMark::Unknown => colours.line,
        CellMark::Drop => colours.err,
        // An absence of the phone is not a fault: the cell stays muted.
        CellMark::Away => colours.muted,
    }
}

/// The colour of a cell of an hour strip that lies inside the window of its
/// tile. A cell outside the window has no colour of its own: it is drawn as
/// a low hairline, see `hour_cells`.
fn cell_colour(colours: &Colours, cell: &Cell) -> egui::Color32 {
    mark_colour(colours, cell.mark)
}

/// The height of the hairline of a cell outside the window of its tile.
const OUTSIDE_CELL_HEIGHT: f32 = 2.0;

/// The height of the hour strip under the tiles, in pixels.
const HOURS_STRIP_HEIGHT: f32 = 20.0;

/// The gap between two cells of an hour strip, in pixels.
const CELL_GAP: f32 = 2.0;

/// The corner radius of a cell of an hour strip, in pixels.
const CELL_RADIUS: f32 = 2.0;

/// The four signal bars of the signal tile.
fn tile_levels(ui: &mut egui::Ui, colours: &Colours, levels: &Levels) {
    let metrics = theme::metrics();
    let width = metrics.spacing.xs;
    let gap = metrics.spacing.xs / 2.0;
    let height = metrics.line_height.caption;
    let count = f32::from(levels.max);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width * count + gap * (count - 1.0), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    for index in 0..levels.max {
        let x = rect.left() + f32::from(index) * (width + gap);
        let step = (f32::from(index) + 1.0) / count;
        let bar = egui::Rect::from_min_max(
            egui::pos2(x, rect.bottom() - height * step),
            egui::pos2(x + width, rect.bottom()),
        );
        let colour = if index < levels.level {
            colours.fg
        } else {
            colours.line
        };
        painter.rect_filled(bar, metrics.spacing.xs / 2.0, colour);
    }
}

/// The drawing under the number of a tile: a line, a two-part bar or an hour
/// strip.
fn tile_trend(ui: &mut egui::Ui, colours: &Colours, trend: &Trend) {
    let metrics = theme::metrics();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(width, metrics.line_height.caption),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    match &trend.kind {
        TrendKind::Line { points } => {
            if points.len() < 2 {
                return;
            }
            let mut min = points[0];
            let mut max = points[0];
            for point in points {
                min = min.min(*point);
                max = max.max(*point);
            }
            if let Some(reference) = trend.reference {
                min = min.min(reference);
                max = max.max(reference);
            }
            // The line keeps 2 px of air above and below it. A flat line sits
            // in the middle of the rect instead of on its edge.
            let plot = rect.shrink2(egui::vec2(0.0, 2.0));
            let flat = max == min;
            // The span is at least 1 only to avoid a division by zero.
            let span = (max - min).max(1) as f32;
            let to_y = |value: i64| -> f32 {
                if flat {
                    rect.center().y
                } else {
                    plot.bottom() - ((value - min) as f32 / span) * plot.height()
                }
            };
            let step = rect.width() / (points.len() - 1) as f32;
            let line: Vec<egui::Pos2> = points
                .iter()
                .enumerate()
                .map(|(index, point)| egui::pos2(rect.left() + index as f32 * step, to_y(*point)))
                .collect();
            painter.add(egui::Shape::line(line, egui::Stroke::new(2.0, colours.fg)));
            if let Some(reference) = trend.reference {
                let y = to_y(reference);
                dashed_line(
                    painter,
                    egui::pos2(rect.left(), y),
                    egui::pos2(rect.right(), y),
                    egui::Stroke::new(1.0, colours.wait),
                );
            }
        }
        TrendKind::Split { down, up } => {
            let total = down + up;
            if total == 0 {
                return;
            }
            let down_width = rect.width() * (*down as f32 / total as f32);
            let half = metrics.spacing.sm / 2.0;
            let bar = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.center().y - half),
                egui::pos2(rect.right(), rect.center().y + half),
            );
            painter.rect_filled(bar, metrics.spacing.xs / 2.0, colours.line);
            painter.rect_filled(
                egui::Rect::from_min_max(bar.min, egui::pos2(rect.left() + down_width, bar.max.y)),
                metrics.spacing.xs / 2.0,
                colours.primary,
            );
        }
        TrendKind::Fill { value, max } => {
            if *max == 0 {
                return;
            }
            let filled = rect.width() * (*value as f32 / *max as f32).clamp(0.0, 1.0);
            let half = metrics.spacing.sm / 2.0;
            let bar = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.center().y - half),
                egui::pos2(rect.right(), rect.center().y + half),
            );
            painter.rect_filled(bar, metrics.spacing.xs / 2.0, colours.line);
            painter.rect_filled(
                egui::Rect::from_min_max(bar.min, egui::pos2(rect.left() + filled, bar.max.y)),
                metrics.spacing.xs / 2.0,
                colours.primary,
            );
        }
        TrendKind::Hours { cells } => {
            hour_cells(ui, colours, cells, rect);
        }
    }
}

/// The cells of an hour strip: one per hour, in the order of the list.
fn hour_cells(ui: &mut egui::Ui, colours: &Colours, cells: &[Cell], rect: egui::Rect) {
    if cells.is_empty() {
        return;
    }
    // The cells are placed by their place in the list, not by the hour they
    // carry: the strip of the drops runs over the last 24 hours, so its first
    // cell is the oldest hour and not the hour of the day.
    let count = cells.len() as f32;
    let cell_width = (rect.width() - CELL_GAP * (count - 1.0)) / count;
    let painter = ui.painter();
    for (index, cell) in cells.iter().enumerate() {
        let x = rect.left() + index as f32 * (cell_width + CELL_GAP);
        if !cell.in_scope {
            // The hour is not counted by the tile: it is drawn fainter than
            // an hour without a verdict, as a low hairline in the line
            // colour, so it never looks like an hour of the window (an
            // absence, a drop) and the strip never shows a red hour the
            // number does not count.
            let hairline = egui::Rect::from_min_max(
                egui::pos2(x, rect.bottom() - OUTSIDE_CELL_HEIGHT),
                egui::pos2(x + cell_width, rect.bottom()),
            );
            painter.rect_filled(hairline, 0.0, colours.line);
            continue;
        }
        let bar = egui::Rect::from_min_max(
            egui::pos2(x, rect.top()),
            egui::pos2(x + cell_width, rect.bottom()),
        );
        painter.rect_filled(bar, CELL_RADIUS, cell_colour(colours, cell));
    }
}

/// A dashed line. The egui of this version has no dashed stroke, so the line
/// is drawn in pieces.
fn dashed_line(painter: &egui::Painter, from: egui::Pos2, to: egui::Pos2, stroke: egui::Stroke) {
    let dash: f32 = 4.0;
    let gap: f32 = 3.0;
    let length = (to - from).length();
    if length <= 0.0 {
        return;
    }
    let direction = (to - from) / length;
    let mut travelled: f32 = 0.0;
    while travelled < length {
        let end = (travelled + dash).min(length);
        painter.line_segment(
            [from + direction * travelled, from + direction * end],
            stroke,
        );
        travelled = end + gap;
    }
}

/// The third-party components shown on the notices page. The wording is the
/// licence family, not a copied notice; the full texts ship with the source.
const THIRD_PARTY_NOTICES: &[(&str, &str)] = &[
    (
        "egui / eframe",
        "The immediate-mode UI and its platform shell. MIT OR Apache-2.0.",
    ),
    (
        "serde / toml",
        "State, flag and control-channel (de)serialisation. MIT OR Apache-2.0.",
    ),
    (
        "ksni",
        "The status-notifier tray item. MIT.",
    ),
    (
        "WireGuard tooling",
        "The tunnel itself is WireGuard; Jafnify only drives it. GPL-2.0.",
    ),
    (
        "Inter",
        "The laptop UI face: Regular for body and captions, SemiBold for values and titles. SIL Open Font License 1.1.",
    ),
    (
        "JetBrains Mono",
        "The connection-log face. SIL Open Font License 1.1.",
    ),
];

/// A settings group heading: a small muted caption with air above it.
fn settings_heading(ui: &mut egui::Ui, text: scale::TextSizes, colours: &Colours, title: &str) {
    ui.add_space(theme::metrics().spacing.sm);
    uppercase_label(ui, title, text.caption, colours.muted);
    ui.add_space(theme::metrics().spacing.xs);
}

/// One flat settings row: the title on the left, the control on the right and
/// a short muted explanation under both, with a thin separator after it.
fn settings_row<R>(
    ui: &mut egui::Ui,
    text: scale::TextSizes,
    colours: &Colours,
    title: &str,
    help: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let control = ui
        .horizontal(|ui| {
            ui.label(egui::RichText::new(title).size(text.body).color(colours.fg));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| add(ui)).inner
        })
        .inner;
    ui.label(
        egui::RichText::new(help)
            .size(text.caption)
            .color(colours.muted),
    );
    ui.add_space(theme::metrics().spacing.xs);
    ui.separator();
    control
}

/// A read-only settings row: a title on the left, a value on the right, a
/// short muted line under both and a separator. The value is not a control:
/// it is a fact, drawn in the same row language as the switches.
fn settings_value_row(
    ui: &mut egui::Ui,
    text: scale::TextSizes,
    colours: &Colours,
    title: &str,
    value: &str,
    help: &str,
) {
    settings_row(ui, text, colours, title, help, |ui| {
        ui.label(egui::RichText::new(value).size(text.body).color(colours.fg));
    });
}

/// A settings row with only a control on the right and no muted help line:
/// the slot rows repeat too often for a sentence under every one.
fn settings_control_row<R>(
    ui: &mut egui::Ui,
    text: scale::TextSizes,
    colours: &Colours,
    title: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let control = ui
        .horizontal(|ui| {
            ui.label(egui::RichText::new(title).size(text.body).color(colours.fg));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| add(ui)).inner
        })
        .inner;
    ui.add_space(theme::metrics().spacing.xs);
    ui.separator();
    control
}

/// A row that leads to another page: the title on the left, a `>` on the
/// right, a muted line under both and a separator. Returns true when the
/// title was clicked.
fn settings_link_row(
    ui: &mut egui::Ui,
    text: scale::TextSizes,
    colours: &Colours,
    title: &str,
    help: &str,
) -> bool {
    // The whole row opens the page: the title, the help line, the arrow and
    // the space between them. The labels are not selectable, so a click on
    // the help line opens the page instead of selecting its text.
    let row = ui.scope(|ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(title).size(text.body).color(colours.fg))
                    .selectable(false),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(">")
                            .size(text.body)
                            .color(colours.muted),
                    )
                    .selectable(false),
                );
            });
        });
        ui.add(
            egui::Label::new(
                egui::RichText::new(help)
                    .size(text.caption)
                    .color(colours.muted),
            )
            .selectable(false),
        );
    });
    // The row spans the full width of the page, whatever the text width.
    let full = ui.max_rect();
    let rect = egui::Rect::from_min_max(
        egui::pos2(full.left(), row.response.rect.top()),
        egui::pos2(full.right(), row.response.rect.bottom()),
    )
    .expand2(egui::vec2(0.0, 2.0));
    let response = ui
        .interact(rect, ui.scope_id().with(("settings_link_row", title)), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(4),
            egui::Stroke::new(1.0, colours.muted.gamma_multiply(0.5)),
            egui::StrokeKind::Outside,
        );
    }
    ui.add_space(theme::metrics().spacing.xs);
    ui.separator();
    response.clicked()
}

fn state_from(raw: &str) -> ConnectionState {
    match raw {
        "searching" => ConnectionState::Searching,
        "pairing" => ConnectionState::Pairing,
        "connecting" => ConnectionState::Connecting,
        "connected" => ConnectionState::Connected,
        "retry" => ConnectionState::Retry,
        _ => ConnectionState::Idle,
    }
}

/// A segmented choice. The theme forces every text to `fg`, which on the
/// selected (`primary`) fill is unreadable in the dark theme; the selected
/// label uses `primary_on`, the token made for text on `primary`.
fn choice(ui: &mut egui::Ui, colours: &Colours, selected: bool, label: &str) -> egui::Response {
    let colour = if selected {
        colours.primary_on
    } else {
        colours.fg
    };
    ui.selectable_label(selected, egui::RichText::new(label).color(colour))
}

/// One Details section: a small caps title and a two-column grid, muted
/// labels and plain values (the phone's Details table, the same order).
fn detail_section(
    ui: &mut egui::Ui,
    colours: &Colours,
    text: scale::TextSizes,
    section: &jafnify_linux::details::DetailSection,
    salt: (&str, usize),
) {
    let metrics = theme::metrics();
    if !section.title.is_empty() {
        uppercase_label(ui, &section.title.to_uppercase(), text.caption, colours.muted);
        ui.add_space(metrics.spacing.xs);
    }
    egui::Grid::new(salt)
        .num_columns(2)
        .spacing([metrics.spacing.lg, metrics.spacing.xs])
        .min_col_width(DETAIL_LABEL_WIDTH)
        .show(ui, |ui| {
            for row in &section.rows {
                ui.label(egui::RichText::new(&row.label).color(colours.muted));
                let colour = if row.warn {
                    colours.wait_text
                } else {
                    colours.fg
                };
                ui.add(egui::Label::new(egui::RichText::new(&row.value).color(colour)).wrap());
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod link_row_tests {
    use super::*;

    /// Draw one link row, then click at a point picked from its laid-out box.
    fn click_row_at(pick: impl Fn(egui::Rect) -> egui::Pos2) -> bool {
        let ctx = egui::Context::default();
        ctx.set_zoom_factor(scale::ZOOM);
        let text = scale::TextSizes::new(1.0);
        let colours = Colours::for_theme(Theme::Dark);
        let row_rect = std::cell::Cell::new(egui::Rect::NOTHING);
        let clicked = std::cell::Cell::new(false);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(544.0, 850.0));
        let frame = |events: Vec<egui::Event>, record: bool| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let top = ui.cursor().min;
                let hit = settings_link_row(
                    ui,
                    text,
                    &colours,
                    "Pairing",
                    "Pair a new phone or forget the paired one.",
                );
                if record {
                    row_rect.set(egui::Rect::from_min_max(
                        top,
                        egui::pos2(ui.max_rect().right(), ui.cursor().min.y),
                    ));
                }
                clicked.set(clicked.get() | hit);
            });
            output.textures_delta.clear();
        };
        // The first frame settles the zoom and the screen size.
        frame(Vec::new(), false);
        frame(Vec::new(), true);
        // Events are in screen points; the row was laid out in zoomed points.
        let at = pick(row_rect.get()) * scale::ZOOM;
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(at)], false);
        frame(vec![press(true)], false);
        frame(vec![press(false)], false);
        frame(Vec::new(), false);
        clicked.get()
    }

    #[test]
    fn a_click_on_the_help_line_opens_the_page() {
        // The help line is the lower part of the row.
        assert!(click_row_at(|r| egui::pos2(r.left() + 40.0, r.top() + r.height() * 0.55)));
    }

    #[test]
    fn a_click_on_the_arrow_side_opens_the_page() {
        assert!(click_row_at(|r| egui::pos2(r.right() - 6.0, r.top() + 10.0)));
    }

    #[test]
    fn a_click_in_the_middle_of_the_row_opens_the_page() {
        assert!(click_row_at(|r| egui::pos2(r.center().x, r.top() + 10.0)));
    }

    #[test]
    fn a_click_below_the_row_does_not_open_the_page() {
        assert!(!click_row_at(|r| egui::pos2(r.center().x, r.bottom() + 60.0)));
    }
}
