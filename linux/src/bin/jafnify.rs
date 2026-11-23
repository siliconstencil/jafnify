use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;
use jafnify_core::state::Role;
use jafnify_core::ConnectionState;
use jafnify_linux::app::{App, AppBuilder, UiEvent, IFACE};
use jafnify_linux::applog;
use jafnify_linux::backend::{interface_present_at, interface_state, InterfaceState, WgBackend};
use jafnify_linux::cli::{Cli, Command, FirewallCommand, KillSwitchCommand};
use jafnify_linux::control::{self, Request, Response};
use jafnify_linux::firewall::rule_lines;
use jafnify_linux::root::{package_owning, RootCommands};
use jafnify_linux::view::{
    details_line, device_line, forgotten_line, peer_line, view, ViewInput, POLKIT_NOTE,
};
use jafnify_linux::APP_DIR;

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR)
}

fn app_builder(dir: PathBuf, logger: Option<applog::Logger>) -> AppBuilder {
    // Only the tray daemon owns sessions.toml. A CLI command may run while
    // the daemon is alive, so its App never writes the session history.
    AppBuilder::new(dir)
        .logger(logger)
        .diag_path(applog::diag_path_unless_test())
        .session_history_owner(false)
}

fn main() {
    let cli = Cli::parse();
    if matches!(
        cli.command,
        Command::Status | Command::Peers | Command::Hours
    ) {
        // A read-only report whose reader went away (`| head`) ends quietly,
        // like other Unix tools, instead of panicking on a broken pipe. Only
        // these commands: the tray must survive a client closing its socket.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn connected_now() -> bool {
    // A live daemon knows the state; the lab runs the joiner headless, so
    // fall back to the tunnel interface as the Connected signal.
    match control::request(&Request::Status) {
        Ok(Response::State(snapshot)) => snapshot.state == "connected",
        _ => match WgBackend::new().status(IFACE) {
            Ok(status) => status.interface_up,
            Err(_) => interface_present_at(Path::new("/sys/class/net"), IFACE),
        },
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let dir = config_dir();
    let logger = applog::open_default_unless_test();
    if matches!(
        cli.command,
        Command::Start { .. } | Command::Join { .. } | Command::Pair { .. }
    ) {
        ensure_terms(&dir, cli.accept_terms)?;
    }
    match cli.command {
        Command::Status => {
            match control::request(&Request::Status) {
                Ok(Response::State(snapshot)) => {
                    println!("{}", device_line(snapshot.paired_name.as_deref()));
                    println!("{}", snapshot.status_text);
                    if let Some(secs) = snapshot.connected_secs {
                        println!(
                            "Connected for: {}",
                            jafnify_core::ui::ui_text::format_elapsed(secs)
                        );
                    }
                    if let Some(note) = &snapshot.dns_override {
                        println!("{note}");
                    }
                    // The same notes as the local path, from the daemon's state.
                    if snapshot.kill_switch {
                        println!("{}", jafnify_linux::view::KILL_SWITCH_NOTE);
                    }
                    if snapshot.polkit_note {
                        println!("{POLKIT_NOTE}");
                    }
                    print_interface_lines();
                }
                _ => {
                    let app = app_builder(dir, logger).build();
                    println!("{}", device_line(app.paired_name().as_deref()));
                    let v = view(&ViewInput::from_app(&app));
                    println!("{}", v.status);
                    if let Some(note) = app.host_dns_override() {
                        println!("{note}");
                    }
                    if let Some(note) = &v.kill_switch_note {
                        println!("{note}");
                    }
                    if v.polkit_note {
                        println!("{POLKIT_NOTE}");
                    }
                    print_interface_lines();
                }
            }
            Ok(())
        }
        Command::LinkTest => {
            let result = jafnify_linux::link_test::run_default(connected_now());
            println!("{}", result.line());
            Ok(())
        }
        Command::PhoneStats => {
            if !connected_now() {
                println!("{}", jafnify_core::phone_stats::CONNECT_FIRST_LINE);
                return Ok(());
            }
            let mut transport = jafnify_linux::phone_stats::UdpTransport::new();
            match jafnify_linux::phone_stats::run_once(&mut transport, true) {
                Ok(Some(summary)) => println!("{}", summary.stats.to_response()),
                _ => println!("{}", jafnify_core::phone_stats::UNAVAILABLE_LINE),
            }
            Ok(())
        }
        Command::Drops => {
            match control::request(&Request::GetDrops) {
                Ok(Response::Drops { summary, rows }) => {
                    println!("{summary}");
                    for row in rows {
                        println!("{row}");
                    }
                }
                _ => {
                    // The daemon is not running: read the file read-only.
                    let app = app_builder(dir, logger).build();
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64);
                    let offset = applog::local_offset_minutes(now_ms);
                    let records = app.drop_records();
                    println!("{}", jafnify_core::drop_history::summary(&records, now_ms));
                    for record in records.iter().rev().take(20) {
                        println!("{}", record.detail(offset));
                    }
                }
            }
            Ok(())
        }
        Command::Hours => {
            match control::request(&Request::GetGoodHours) {
                Ok(Response::GoodHours { lines }) => {
                    for line in lines {
                        println!("{line}");
                    }
                }
                _ => {
                    // The daemon is not running: read the file read-only.
                    let app = app_builder(dir.clone(), logger).build();
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64);
                    let offset = applog::local_offset_minutes(now_ms);
                    // The switch decides: without it there is nothing to show.
                    let flags = jafnify_linux::flags::FlagStore::load(
                        &dir.join(jafnify_linux::flags::FILE_NAME),
                    );
                    let lines = jafnify_linux::flags::good_hours_lines(
                        flags.get("good_hours"),
                        app.good_hours_lines(now_ms, offset),
                    );
                    for line in lines {
                        println!("{line}");
                    }
                }
            }
            Ok(())
        }
        Command::Peers => {
            let app = app_builder(dir, logger).build();
            for p in app.peers() {
                println!("{}", peer_line(&p.name, &p.id));
            }
            Ok(())
        }
        Command::Stop => {
            let mut app = app_builder(dir, logger).build();
            app.stop().map_err(|e| e.to_string())
        }
        Command::Fallback => {
            let mut app = app_builder(dir, logger).build();
            app.clear_stale_routing();
            Ok(())
        }
        Command::Start { code } => {
            let mut app = app_builder(dir, logger).build();
            start_with_code_or_pair(&mut app, code)?;
            headless_loop(&mut app)
        }
        Command::Join { code } => {
            let mut app = app_builder(dir, logger).build();
            app.set_role(Role::Joiner);
            start_with_code_or_pair(&mut app, code)?;
            headless_loop(&mut app)
        }
        Command::Forget => {
            let mut app = app_builder(dir, logger).build();
            let name = app.paired_name();
            app.forget().map_err(|e| e.to_string())?;
            println!("{}", forgotten_line(name.as_deref()));
            Ok(())
        }
        Command::Probe { code, seconds } => {
            let outcome = jafnify_linux::probe::run(
                std::path::Path::new("ip"),
                &code,
                seconds,
                std::path::Path::new(jafnify_linux::firewall::UFW_CONF),
                |line| println!("{line}"),
            )?;
            for line in &outcome.lines {
                println!("{line}");
            }
            if outcome.ok {
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        Command::Firewall { action } => {
            let mut app = app_builder(dir, logger).build();
            let iface = app
                .lan_iface()
                .ok_or("no LAN interface found - connect to the hotspot or Wi-Fi first")?;
            match action {
                FirewallCommand::Rule => {
                    for line in rule_lines(&iface) {
                        println!("{line}");
                    }
                    Ok(())
                }
                FirewallCommand::Allow { yes } => {
                    if !app.ufw_active() {
                        return Err("ufw is not enabled on this computer; `jafnify firewall \
                                    rule` prints the rule to add to another firewall"
                            .to_string());
                    }
                    if !yes && !consent(&iface) {
                        println!("Nothing changed.");
                        return Ok(());
                    }
                    let iface = app.allow_firewall().map_err(|e| e.to_string())?;
                    println!("Rule added for {iface}. The firewall stays on.");
                    Ok(())
                }
            }
        }
        Command::KillSwitch { action } => {
            let mut app = app_builder(dir, logger).build();
            let on = matches!(action, KillSwitchCommand::On);
            app.set_kill_switch(on).map_err(|e| e.to_string())?;
            println!(
                "{}",
                if on {
                    "Kill switch on. It applies from the next connection."
                } else {
                    "Kill switch off."
                }
            );
            Ok(())
        }
        Command::Pair { code } => {
            let mut app = app_builder(dir, logger).build();
            app.start(&code).map_err(|e| e.to_string())?;
            // pair = start + accept own code immediately (mutual model:
            // both ends enter the same code, so it always matches)
            app.accept_code(&code).map_err(|e| e.to_string())?;
            headless_loop(&mut app)
        }
        Command::Tray => jafnify_linux::daemon::run(),
        Command::InstallHelper => install_helper(),
        Command::Uninstall => uninstall(),
    }
}

/// The interface lines shared by the daemon-backed and local status paths.
fn print_interface_lines() {
    let backend = WgBackend::new();
    let present = interface_present_at(Path::new("/sys/class/net"), IFACE);
    match backend.status(IFACE) {
        Ok(s) if s.interface_up => {
            println!("public key: {}", s.public_key.as_deref().unwrap_or("-"));
            println!("listen port: {}", s.listen_port.unwrap_or(0));
        }
        // `wg show` is unprivileged-hostile; presence decides.
        _ if interface_state(false, present) == InterfaceState::Up => {
            println!("interface: up");
        }
        _ => println!("interface: down"),
    }
}

/// Remove the installed paths and the per-user rule in one root call. The
/// paired state under the user's config directory is intentionally kept.
fn uninstall() -> Result<(), String> {
    // A package manager owns its files. Removing them behind dpkg's back
    // leaves the package installed with missing files, so refuse instead.
    if let Some(package) = package_owning(
        Path::new(jafnify_linux::root::HELPER_PATH),
        Path::new("/var/lib/dpkg/info"),
        jafnify_linux::flavor::NAME,
        Path::new("dpkg-query"),
    ) {
        return Err(format!(
            "Installed as a package: remove it with sudo apt remove {package}"
        ));
    }
    // A root process has no user bus; calling `systemctl --user` there only
    // prints "Failed to connect to user scope bus".
    let as_root = unsafe { libc::geteuid() } == 0;
    if let Some(config) = dirs::config_dir() {
        let unit = config
            .join("systemd/user")
            .join(jafnify_linux::flavor::UNIT);
        if !as_root {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "stop", jafnify_linux::flavor::UNIT])
                .status();
            let _ = jafnify_linux::autostart::disable(&unit, Path::new("systemctl"));
        }
        let _ = std::fs::remove_file(&unit);
    }
    let root = RootCommands::new(IFACE);
    let out = root
        .run(root.uninstall_command())
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    println!("Jafnify was uninstalled. The paired state and log were kept.");
    Ok(())
}

/// Install the helper, policy, desktop entry and icons in one root call.
/// The sources are embedded in this binary; the helper binary must be built
/// next to it (`cargo build --release` builds both).
fn install_helper() -> Result<(), String> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or("cannot find the jafnify directory")?;
    let sibling_helper = parent.join("jafnify-helper");
    let helper = if sibling_helper.is_file() {
        sibling_helper
    } else {
        PathBuf::from(jafnify_linux::root::HELPER_PATH)
    };
    let sibling_route = parent.join("jafnify-route");
    let route = if sibling_route.is_file() {
        sibling_route
    } else {
        PathBuf::from(jafnify_linux::root::ROUTE_PATH)
    };
    let gui = parent.join(jafnify_linux::flavor::flavored("jafnify-gui"));
    for path in [&exe, &gui, &helper, &route] {
        if !path.is_file() {
            return Err(format!(
                "{} is missing - build all binaries first (cargo build --release)",
                path.display()
            ));
        }
    }
    let dir = std::env::temp_dir().join(format!("jafnify-install-{}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    let policy = dir.join("org.jafnify.policy");
    let desktop = dir.join("jafnify.desktop");
    let icon = dir.join("jafnify.svg");
    let symbolic = dir.join("jafnify-symbolic.svg");
    fs::write(
        &policy,
        jafnify_linux::flavor::flavored(include_str!("../../../assets/polkit/org.jafnify.policy")),
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        &desktop,
        jafnify_linux::flavor::flavored(include_str!("../../../assets/linux/jafnify.desktop")),
    )
    .map_err(|e| e.to_string())?;
    fs::write(&icon, include_str!("../../../assets/icon/jafnify.svg"))
        .map_err(|e| e.to_string())?;
    fs::write(
        &symbolic,
        include_str!("../../../assets/icon/jafnify-symbolic.svg"),
    )
    .map_err(|e| e.to_string())?;
    // Install also rewrites the per-user rule, so an upgrade from a rule that
    // only granted the tunnel action gains the route grant. The rule is the
    // adoption consent: install only REPLACES an existing rule (checked in the
    // root step), never creates one. Under sudo the rule names the sudo
    // caller, never root, so an upgrade cannot hand the grant to another user.
    let rules = match jafnify_linux::polkit::rule_owner_for_install() {
        Some(user) => {
            let path = dir.join(jafnify_linux::flavor::RULE_FILE);
            fs::write(&path, jafnify_linux::polkit::rule_text(&user)?)
                .map_err(|e| e.to_string())?;
            Some(path)
        }
        None => None,
    };

    let root = RootCommands::new(IFACE);
    let cmd = root.install_helper_command(jafnify_linux::root::InstallPaths {
        jafnify: &exe,
        gui: &gui,
        helper: &helper,
        route: &route,
        policy: &policy,
        desktop: &desktop,
        icon: &icon,
        symbolic: &symbolic,
        rules: rules.as_deref(),
    });
    let out = root.run(cmd).map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(&dir);
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    println!(
        "Jafnify installed in /usr/local/bin with the helper and polkit action. \
         The first adoption asks once and installs a per-user rule; later connects do not prompt."
    );
    Ok(())
}

/// Sharing or joining starts only after the terms are accepted (stored once in
/// the config directory). Asks on a terminal; refuses otherwise without the flag.
fn ensure_terms(dir: &std::path::Path, accept_flag: bool) -> Result<(), String> {
    use jafnify_core::terms::{is_accepted, record_acceptance, DISCLAIMER};
    use jafnify_linux::terms::{cli_gate, Gate};
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let record = || {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        record_acceptance(dir, now.as_secs()).map_err(|e| e.to_string())
    };
    match cli_gate(is_accepted(dir), accept_flag, interactive) {
        Gate::Proceed => Ok(()),
        Gate::Record => record(),
        Gate::Ask => {
            println!("{DISCLAIMER}\n");
            print!("Type y to accept these terms and continue [y/N] ");
            let _ = std::io::stdout().flush();
            let mut answer = String::new();
            let yes = std::io::stdin().read_line(&mut answer).is_ok()
                && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes");
            if yes {
                record()
            } else {
                Err("The terms were not accepted - nothing was started".to_string())
            }
        }
        Gate::Refuse => Err(
            "Read the terms with `jafnify --help`, then run again with --accept-terms".to_string(),
        ),
    }
}

/// Show the one rule and ask. Anything but y/yes (or no answer) is a no.
fn consent(iface: &str) -> bool {
    println!("This adds ONE firewall rule (your password is needed):");
    println!("  {}", &rule_lines(iface)[0]["ufw: ".len()..]);
    println!("Nothing else changes and the firewall stays on.");
    print!("Add it? [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_lowercase().as_str(), "y" | "yes")
}

/// A code (flag or JAFNIFY_CODE) pairs; without one the paired device is used.
fn start_with_code_or_pair(app: &mut App, code: Option<String>) -> Result<(), String> {
    match code.or_else(|| std::env::var("JAFNIFY_CODE").ok()) {
        Some(code) => app.start(&code),
        None => app.start_known(),
    }
    .map_err(|e| e.to_string())
}

/// Prints one line whenever the status sentence changes (the phone's and the
/// window's sentences), with the technical reason indented under it. A
/// refused password prompt ends the command: the CLI only asks once.
fn headless_loop(app: &mut App) -> Result<(), String> {
    jafnify_linux::stop::install();
    app.clear_stale_routing();
    if let Some(note) = app.host_dns_override() {
        println!("{note}");
    }
    let mut last = String::new();
    let mut note_shown = false;
    let mut no_peer = false;
    loop {
        if jafnify_linux::stop::requested() {
            // A service stop must not leave the dead tunnel's routing behind.
            // An attempt that never reached Connected ends with its tunnel too:
            // an interrupted join must not leave an orphan jafn0 behind for the
            // tray to call another Jafnify connection.
            match jafnify_linux::stop::shutdown_action(app.state() == ConnectionState::Connected) {
                jafnify_linux::stop::ShutdownAction::FallBack => {
                    let _ = app.fall_back_routing();
                }
                jafnify_linux::stop::ShutdownAction::TearDown => {
                    if let Err(e) = app.stop() {
                        eprintln!("jafnify: stop: {e}");
                    }
                }
            }
            return Ok(());
        }
        // The attempt hint is armed before the blocking up call; read it here
        // so it is visible even when the first tick is the call that raises
        // the tunnel.
        let pending = ViewInput::from_app(app);
        if view(&pending).polkit_note && !note_shown {
            println!("{POLKIT_NOTE}");
            note_shown = true;
        }
        match app.tick().map_err(|e| e.to_string())? {
            Some(UiEvent::NoPeerFound) => no_peer = true,
            Some(UiEvent::PermissionDenied) => {
                return Err("Permission was denied - try again".to_string());
            }
            Some(UiEvent::RootError(e)) => println!("{e}"),
            _ => {}
        }
        if app.state() != ConnectionState::Searching {
            no_peer = false;
        }
        let mut input = ViewInput::from_app(app);
        input.no_peer = no_peer;
        let v = view(&input);
        let line = v.status;
        if line != last {
            println!("{line}");
            if let Some(reason) = &input.retry_reason {
                println!("{}", details_line(reason));
            }
            if let Some(help) = &v.help {
                println!("{}", details_line(help));
            }
            if let Some(note) = &v.kill_switch_note {
                println!("{note}");
            }
            last = line;
        }
        if v.polkit_note && !note_shown {
            println!("{POLKIT_NOTE}");
            note_shown = true;
        } else if !v.polkit_note {
            note_shown = false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
