pub mod joiner;
pub mod probe;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jlong, jlongArray, jstring};
use jni::JNIEnv;

struct App {
    joiner: joiner::Joiner,
    probe: probe::ProbeRun,
    /// The ring of the phone's own series: five minutes, in memory only. It
    /// lives as long as the service does.
    trends: Mutex<jafnify_core::tiles::Trends>,
}

impl App {
    pub fn new() -> App {
        App {
            joiner: joiner::Joiner::new(),
            probe: probe::ProbeRun::new(),
            trends: Mutex::new(jafnify_core::tiles::Trends::new()),
        }
    }
}

/// Live instances by opaque handle. Java never sees a pointer, so a zero or
/// stale handle is rejected instead of dereferenced, and a second destroy is
/// a no-op instead of a double free.
struct Registry {
    next: u64,
    apps: HashMap<u64, Arc<App>>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            next: 1,
            apps: HashMap::new(),
        })
    })
}

/// Register a new instance and return its handle (> 0).
fn create_app() -> jlong {
    let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    let handle = reg.next;
    // Never hand out zero: it means "no instance" to Java.
    reg.next = reg.next.saturating_add(1).max(1);
    reg.apps.insert(handle, Arc::new(App::new()));
    handle as jlong
}

/// Free an instance. A zero or stale handle is a no-op, so a second destroy
/// cannot free twice.
fn destroy_app(handle: jlong) {
    if handle <= 0 {
        return;
    }
    let app = {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        reg.apps.remove(&(handle as u64))
    };
    if let Some(app) = app {
        app.joiner.stop();
        drop(app);
    }
}

/// Run `f` on the live instance `handle`; None when the handle is zero or not
/// live. The registry lock is held only to clone the `Arc`, never for the
/// call: JNI calls from different threads (UI polls, the service, a slow
/// start) must not wait for each other. A concurrent destroy only drops the
/// registry's reference; the instance lives until this call ends.
fn with_app<R>(handle: jlong, f: impl FnOnce(&App) -> R) -> Option<R> {
    if handle <= 0 {
        return None;
    }
    let app = {
        let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        reg.apps.get(&(handle as u64)).cloned()
    }?;
    Some(f(&app))
}

/// A JNI call that fails must never panic across the FFI boundary. Clear the
/// pending Java exception and use `fallback` instead.
fn jni_or<T>(
    env: &mut JNIEnv,
    call: impl FnOnce(&mut JNIEnv) -> Result<T, jni::errors::Error>,
    fallback: T,
) -> T {
    match call(env) {
        Ok(value) => value,
        Err(_) => {
            let _ = env.exception_clear();
            fallback
        }
    }
}

/// Read a Java string; a failure reads as empty and clears any pending
/// exception, so the next JNI call cannot trip over it.
fn jni_string(env: &mut JNIEnv, value: &JString) -> String {
    match env.get_string(value) {
        Ok(text) => text.into(),
        Err(_) => {
            let _ = env.exception_clear();
            String::new()
        }
    }
}

/// Build a Java string, or null when the call fails. One helper for every
/// string the JNI surface returns.
fn jni_string_out(env: &mut JNIEnv, text: &str) -> jstring {
    jni_or(
        env,
        |env| env.new_string(text).map(|value| value.into_raw()),
        std::ptr::null_mut(),
    )
}

/// A Rust panic must not cross the JNI boundary: it aborts the whole app.
/// Catch it, log one line, and hand Java the safe default instead.
fn jni_guard<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(_) => {
            tracing::error!("JNI call panicked; returning a safe default");
            fallback
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_create(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    jni_guard(0, create_app)
}

#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_destroy(
    _env: JNIEnv,
    _class: JClass,
    instance: jlong,
) {
    jni_guard((), || destroy_app(instance))
}

#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerStart(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
    ifaces: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let ifaces = jni_string(&mut env, &ifaces);
        let Some(code) = with_app(instance, |app| {
            app.joiner.sharer_start(joiner::discovery_config(&ifaces))
        }) else {
            return jni_string_out(&mut env, "");
        };
        match code {
            Ok(code) => jni_string_out(&mut env, &code),
            Err(e) => jni_string_out(&mut env, &e),
        }
    })
}

/// Where the paired device is kept (the app's private files dir).
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_setStatePath(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
    path: JString,
) {
    jni_guard((), || {
        let path = jni_string(&mut env, &path);
        with_app(instance, |app| app.joiner.set_state_path(path.into()));
    })
}

/// Short name of the paired device, or "" when none is stored.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_pairedName(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let name = with_app(instance, |app| app.joiner.paired_name().unwrap_or_default())
            .unwrap_or_default();
        jni_string_out(&mut env, &name)
    })
}

/// Sharer for the paired device (no code). Empty string = started.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerStartKnown(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
    ifaces: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let ifaces = jni_string(&mut env, &ifaces);
        let ret = with_app(instance, |app| {
            app.joiner
                .sharer_start_known(joiner::discovery_config(&ifaces))
        });
        let text = ret.unwrap_or_else(|| Err("no instance".to_string()));
        jni_string_out(&mut env, &text.err().unwrap_or_default())
    })
}

/// Restarts the sharer on a new interface list without recreating the app.
/// Stays on the current track: the same code while pairing, or the stored
/// pair once one is in use. Empty string = restarted, otherwise the error.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerRestart(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
    ifaces: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let ifaces = jni_string(&mut env, &ifaces);
        let ret = with_app(instance, |app| {
            app.joiner.sharer_restart(joiner::discovery_config(&ifaces))
        });
        let text = ret.unwrap_or_else(|| Err("no instance".to_string()));
        jni_string_out(&mut env, &text.err().unwrap_or_default())
    })
}

/// Stops and deletes the paired device. Empty string = done.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_forgetPeer(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = with_app(instance, |app| app.joiner.forget())
            .unwrap_or_else(|| Err("no instance".to_string()));
        jni_string_out(&mut env, &text.err().unwrap_or_default())
    })
}

#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerStatus(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = with_app(instance, |app| app.joiner.status_with_endpoint()).unwrap_or_default();
        jni_string_out(&mut env, &text)
    })
}

/// The core's words for the idle card, used before the sharer has reported.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_idleStatus(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        jni_string_out(
            &mut env,
            &jafnify_core::snapshot::Snapshot::default().status_line(),
        )
    })
}

/// The line under the status on the first open after the system closed the
/// app while sharing; "" when there is nothing to say. Core wording.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_stoppedLine(
    mut env: JNIEnv,
    _class: JClass,
    reason: jint,
    age_ms: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let reason = jafnify_core::phone_exit::from_android(reason, age_ms);
        jni_string_out(&mut env, jafnify_core::share_notice::stopped_line(reason))
    })
}

/// The one line while the phone shares with notifications off.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_notificationsOffLine(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        jni_string_out(&mut env, jafnify_core::share_notice::NOTIFICATIONS_OFF)
    })
}

/// The About row and the notices screen title.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_thirdPartyNoticesTitle(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        jni_string_out(&mut env, jafnify_core::share_notice::THIRD_PARTY_NOTICES)
    })
}

/// One `flagsMeta` line: id TAB bucket TAB default TAB platforms TAB label
/// TAB help. Labels and help are single-line ASCII in the core table.
fn flag_line(flag: &jafnify_core::flags::FeatureFlag) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}",
        flag.id,
        flag.bucket.key(),
        if flag.default { "true" } else { "false" },
        flag.platforms,
        flag.label,
        flag.help
    )
}

/// The flag table for `platform`, one line per renderable row; "" when none.
/// Unwired rows never reach Java, so a released app cannot show a dead switch.
fn flags_meta_text(table: &[jafnify_core::flags::FeatureFlag], platform: u8) -> String {
    table
        .iter()
        .filter(|flag| jafnify_core::flags::is_rendered(flag, platform))
        .map(flag_line)
        .collect::<Vec<String>>()
        .join("\n")
}

/// The Android flag rows as newline-separated `flagsMeta` lines. Empty until
/// a row is wired.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_flagsMeta(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = flags_meta_text(
            jafnify_core::flags::FLAGS,
            jafnify_core::flags::platform::ANDROID,
        );
        jni_string_out(&mut env, &text)
    })
}

/// One `flagsBucketsMeta` line: key TAB title TAB collapsed(1/0) TAB note.
fn bucket_line(bucket: jafnify_core::flags::Bucket) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        bucket.key(),
        bucket.title(),
        if bucket.collapsed_by_default() {
            "1"
        } else {
            "0"
        },
        bucket.note().unwrap_or("")
    )
}

/// The bucket metadata for Android, one line per bucket in core order.
fn flags_buckets_meta_text() -> String {
    jafnify_core::flags::Bucket::ORDER
        .iter()
        .map(|bucket| bucket_line(*bucket))
        .collect::<Vec<String>>()
        .join("\n")
}

/// The Android bucket titles, collapsed defaults and the Experimental note.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_flagsBucketsMeta(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        jni_string_out(&mut env, &flags_buckets_meta_text())
    })
}

/// The ids of the standard rows, one per line: always on, no switch. The
/// phone reads the stored choice only for the rows this list does not name.
fn standard_flags_text() -> String {
    jafnify_core::flags::FLAGS
        .iter()
        .filter(|flag| jafnify_core::flags::is_standard(flag))
        .map(|flag| flag.id)
        .collect::<Vec<&str>>()
        .join("\n")
}

/// The always-on flag ids for Android, one per line.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_standardFlags(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        jni_string_out(&mut env, &standard_flags_text())
    })
}

/// The placeable phone tiles: id TAB label, one per line.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_homeTileCatalog(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = jafnify_core::home_layout::ANDROID_CATALOG
            .iter()
            .map(|(id, label)| format!("{id}\t{label}"))
            .collect::<Vec<String>>()
            .join("\n");
        jni_string_out(&mut env, &text)
    })
}

/// The default phone home layout, as the saved text.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_homeLayoutDefault(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let layout = jafnify_core::home_layout::Layout::default_for(
            jafnify_core::home_layout::Platform::Android,
        );
        jni_string_out(&mut env, &layout.to_text())
    })
}

/// Normalize a phone layout: the grid word, the wide word and the ids in the
/// order the user wants. A bad grid is the default; an id of another
/// platform is dropped; a short list is filled from the default.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_homeLayoutNormalize(
    mut env: JNIEnv,
    _class: JClass,
    grid: JString,
    wide: JString,
    ids: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let grid = jni_string(&mut env, &grid);
        let wide = jni_string(&mut env, &wide);
        let ids = jni_string(&mut env, &ids);
        let grid = jafnify_core::home_layout::Grid::parse(&grid)
            .unwrap_or(jafnify_core::home_layout::Grid::Eight);
        let layout = jafnify_core::home_layout::Layout {
            grid,
            wide: jafnify_core::home_layout::WideAt::parse(&wide),
            slots: ids
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect(),
        }
        .normalize(jafnify_core::home_layout::Platform::Android);
        jni_string_out(&mut env, &layout.to_text())
    })
}

/// The summary line for a sampler tick with no exit reason. The JNI wrapper
/// calls the `_with_exit` form; the test helpers use this one.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn phone_stats_line(
    up_bps: u64,
    down_bps: u64,
    network: &str,
    signal: i32,
    dbm: i32,
    battery_pct: i32,
    temp_c_x10: i32,
    used_bytes: u64,
    allow_bytes: u64,
    reset_day: i32,
    month_key: u32,
    switches: i32,
    rtt_ms: Option<u32>,
    thermal_status: i32,
) -> String {
    phone_stats_line_with_exit(
        up_bps,
        down_bps,
        network,
        signal,
        dbm,
        battery_pct,
        temp_c_x10,
        used_bytes,
        allow_bytes,
        reset_day,
        month_key,
        switches,
        rtt_ms,
        thermal_status,
        -1,
        false,
        None,
    )
}

/// The summary line, with the session's one-time exit reason when it has one.
#[allow(clippy::too_many_arguments)]
fn phone_stats_line_with_exit(
    up_bps: u64,
    down_bps: u64,
    network: &str,
    signal: i32,
    dbm: i32,
    battery_pct: i32,
    temp_c_x10: i32,
    used_bytes: u64,
    allow_bytes: u64,
    reset_day: i32,
    month_key: u32,
    switches: i32,
    rtt_ms: Option<u32>,
    thermal_status: i32,
    hr30_pct: i32,
    network_permission: bool,
    exit: Option<jafnify_core::phone_exit::ExitReason>,
) -> String {
    let stats = jafnify_core::phone_stats::PhoneStats {
        net: jafnify_core::phone_stats::NetworkKind::parse(network),
        signal: (0..=4).contains(&signal).then_some(signal as u8),
        dbm: i16::try_from(dbm)
            .ok()
            .filter(|value| *value != i16::MIN && (-200..=-2).contains(value)),
        switches: (switches >= 0).then_some(switches as u32),
        rtt_ms: rtt_ms.filter(|&rtt| rtt > 0 && rtt <= 60_000),
        up_bps,
        down_bps,
        battery: (0..=100)
            .contains(&battery_pct)
            .then_some(battery_pct as u8),
        temp_c_x10: i16::try_from(temp_c_x10).ok(),
        thr: (0..=6)
            .contains(&thermal_status)
            .then_some(thermal_status as u8),
        // 0 to 200 percent; a negative value means "unsupported".
        hr30: (0..=200).contains(&hr30_pct).then_some(hr30_pct as u8),
        used_bytes,
        allow_bytes,
        reset_day: reset_day.clamp(0, 31) as u8,
        month: month_key,
        // The WireGuard numbers are added by the responder from the live tunnel.
        wg_loss_pct: None,
        wg_rtt_ms: None,
        // The responder says what this phone takes and how long it has been
        // connected. The network capability comes from the phone's permission.
        caps_link: false,
        caps_net: network_permission,
        sconn_ms: None,
    };
    jafnify_core::phone_exit::with_summary_key(&stats.to_response(), exit)
}

/// The phone's main-screen section, from the last M0 line it sent.
fn phone_section(
    summary_line: &str,
    show_dns: bool,
    show_switches: bool,
    show_battery: bool,
) -> String {
    let Ok(mut stats) =
        jafnify_core::phone_stats::PhoneStats::parse_response(summary_line.trim().as_bytes())
    else {
        return String::new();
    };
    // Each line follows its own switch: no DNS line with the DNS switch off,
    // no "0 switches" without the network report.
    if !show_switches {
        stats.switches = None;
    }
    jafnify_core::phone_stats::main_lines(&stats, show_dns, show_battery).join("\n")
}

/// The phone's own section; empty when the line does not parse.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_phoneSection(
    mut env: JNIEnv,
    _class: JClass,
    summary_line: JString,
    show_dns: jboolean,
    show_switches: jboolean,
    show_battery: jboolean,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let line = jni_string(&mut env, &summary_line);
        let text = phone_section(&line, show_dns != 0, show_switches != 0, show_battery != 0);
        jni_string_out(&mut env, &text)
    })
}

/// What the phone home call needs to build the home.
struct PhoneHomeArgs<'a> {
    summary_line: &'a str,
    usage_lines: &'a str,
    facts_blob: &'a str,
    trends: &'a jafnify_core::tiles::Trends,
    now_ms: i64,
    offset_minutes: i32,
    uptime_secs: Option<u64>,
    state: jafnify_core::tiles::PhoneShareState,
    state_line: Option<&'a str>,
    notice: Option<&'a str>,
    show_dns: bool,
    show_battery: bool,
    /// Whether the phone may read the network generation and the signal.
    network_permission: bool,
    /// The scope of the today tile, in the words of the core.
    today_scope: &'a str,
    /// The scope of the speed tile, in the words of the core.
    speed_scope: &'a str,
    /// The scope of the average tile, in the words of the core.
    average_scope: &'a str,
    /// The saved home layout text; empty means the platform default.
    layout: &'a str,
    /// The sharing session as one line; empty when there is no session.
    session: &'a str,
}

/// The home model of the phone, from what the phone itself knows. The wording
/// lives in the core; this only gathers the facts and hands them over.
fn phone_home_text(args: &PhoneHomeArgs) -> String {
    // The card does not need the phone's measurements: a phone that sends no
    // summary line still says whether it is sharing.
    let stats =
        jafnify_core::phone_stats::PhoneStats::parse_response(args.summary_line.trim().as_bytes())
            .ok();
    let facts = jafnify_core::device_panel::DeviceFacts::parse(args.facts_blob);
    let buckets: Vec<jafnify_core::usage::UsageBucket> = args
        .usage_lines
        .lines()
        .filter_map(|line| jafnify_core::usage::UsageBucket::parse_line(line.trim()).ok())
        .collect();
    let today = jafnify_core::usage::UsageSummary::build(
        &buckets,
        jafnify_core::usage::UsageWindow::Today,
        args.now_ms,
        args.offset_minutes,
    );
    // The scope and the session are the core's words: the phone only hands
    // over the choice it remembers.
    // The core owns the fallback: a word it does not know, or a scope the
    // tile does not have, falls back to the default of that tile.
    let today_scope = jafnify_core::scope::TileScope::for_tile(
        jafnify_core::tiles::ID_TODAY,
        Some(args.today_scope),
    );
    let speed_scope = jafnify_core::scope::TileScope::for_tile(
        jafnify_core::tiles::ID_SPEED,
        Some(args.speed_scope),
    );
    let average_scope = jafnify_core::scope::TileScope::for_tile(
        jafnify_core::tiles::ID_AVERAGE,
        Some(args.average_scope),
    );
    let session = jafnify_core::scope::ScopeSession::parse(args.session);
    let phone_range = joiner::phone_stats_provider().range(args.now_ms);
    let mut model = jafnify_core::tiles::phone_home(&jafnify_core::tiles::PhoneHomeInput {
        state: args.state,
        state_line: args.state_line,
        notice: args.notice,
        // The card counts the sharing, not the phone: the device uptime belongs
        // to the device panel.
        uptime_secs: args.uptime_secs,
        today: Some(&today),
        stats: stats.as_ref(),
        network_permission: args.network_permission,
        trends: args.trends,
        charging: facts.charging,
        battery_saver: facts.battery_saver,
        show_dns: args.show_dns,
        show_battery: args.show_battery,
        today_scope,
        speed_scope,
        average_scope,
        now_ms: args.now_ms,
        session: session.as_ref(),
        range: phone_range.as_ref().map(|(report, trend)| (*report, trend)),
    });
    // The layout owns the slots: one tile per slot, in the order the user
    // chose, with the muted "--" tile for a slot whose data is missing. An
    // empty text keeps the data order, so an older caller still works.
    if !args.layout.trim().is_empty() {
        let layout = jafnify_core::home_layout::Layout::parse(args.layout)
            .unwrap_or_else(|| {
                jafnify_core::home_layout::Layout::default_for(
                    jafnify_core::home_layout::Platform::Android,
                )
            })
            .normalize(jafnify_core::home_layout::Platform::Android);
        model.tiles = jafnify_core::home_layout::arrange(
            &layout,
            jafnify_core::home_layout::Platform::Android,
            model.tiles,
        );
    }
    jafnify_core::tiles::phone_home_text(&model)
}

/// The home model of the phone, in one versioned text block: the header, the
/// card, then one line per tile. "" when the summary line does not parse.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_phoneHome(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
    summary_line: JString,
    usage_lines: JString,
    facts: JString,
    state_line: JString,
    notice: JString,
    now_ms: jlong,
    offset_minutes: jint,
    uptime_secs: jlong,
    state: jint,
    show_dns: jboolean,
    show_battery: jboolean,
    today_scope: JString,
    speed_scope: JString,
    session: JString,
    average_scope: JString,
    layout: JString,
    network_permission: jboolean,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let summary_line = jni_string(&mut env, &summary_line);
        let usage_lines = jni_string(&mut env, &usage_lines);
        let facts = jni_string(&mut env, &facts);
        let state_line = jni_string(&mut env, &state_line);
        let notice = jni_string(&mut env, &notice);
        let today_scope = jni_string(&mut env, &today_scope);
        let speed_scope = jni_string(&mut env, &speed_scope);
        let session = jni_string(&mut env, &session);
        let average_scope = jni_string(&mut env, &average_scope);
        let layout = jni_string(&mut env, &layout);
        let text = with_trends(instance, |trends| {
            phone_home_text(&PhoneHomeArgs {
                summary_line: &summary_line,
                usage_lines: &usage_lines,
                facts_blob: &facts,
                trends,
                now_ms,
                offset_minutes,
                uptime_secs: (uptime_secs >= 0).then_some(uptime_secs as u64),
                state: jafnify_core::tiles::PhoneShareState::from_code(state),
                state_line: (!state_line.is_empty()).then_some(state_line.as_str()),
                notice: (!notice.is_empty()).then_some(notice.as_str()),
                show_dns: show_dns != 0,
                show_battery: show_battery != 0,
                network_permission: network_permission != 0,
                today_scope: &today_scope,
                speed_scope: &speed_scope,
                average_scope: &average_scope,
                layout: &layout,
                session: &session,
            })
        });
        jni_string_out(&mut env, &text)
    })
}

/// The next scope of a tile, in the words of the core. The cycle belongs to
/// the core: the phone only stores the choice. A tile without a scope to
/// change gets an empty word.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_nextTileScope(
    mut env: JNIEnv,
    _class: JClass,
    tile_id: JString,
    scope: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let tile_id = jni_string(&mut env, &tile_id);
        let scope = jni_string(&mut env, &scope);
        if !jafnify_core::scope::TileScope::is_scopeable(&tile_id) {
            return jni_string_out(&mut env, "");
        }
        let next = jafnify_core::scope::TileScope::for_tile(&tile_id, Some(&scope)).next(&tile_id);
        jni_string_out(&mut env, next.word())
    })
}

/// The words that say a tile can be tapped to change its scope. The core
/// owns the words, so the phone does not keep its own copy.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_tileTapHint(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    match env.new_string(jafnify_core::scope::TAP_TO_CHANGE) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Run `f` with the phone's trend ring. A zero or stale instance means the
/// service is not up: the ring is then empty, not a crash.
fn with_trends<R>(instance: jlong, f: impl Fn(&jafnify_core::tiles::Trends) -> R) -> R {
    if let Some(value) = with_app(instance, |app| {
        let trends = app.trends.lock().unwrap_or_else(|err| err.into_inner());
        f(&trends)
    }) {
        return value;
    }
    static EMPTY: OnceLock<Mutex<jafnify_core::tiles::Trends>> = OnceLock::new();
    let empty = EMPTY.get_or_init(|| Mutex::new(jafnify_core::tiles::Trends::new()));
    let trends = empty.lock().unwrap_or_else(|err| err.into_inner());
    f(&trends)
}

/// Epoch milliseconds, or -1 if the system clock is before the epoch.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(-1)
}

/// The best-spot lines of the phone screen: the live line, one line per
/// marked spot and the verdict. The wording is the core's.
fn best_spot_lines(samples: &str, marks: &str) -> String {
    use jafnify_core::best_spot;
    let samples = best_spot::parse_samples(samples);
    let marks = best_spot::parse_marks(marks);
    let mut lines = vec![best_spot::live_line(samples.last())];
    let mut spots: Vec<best_spot::Spot> = Vec::new();
    for mark in &marks {
        match best_spot::spot_at(&samples, *mark) {
            Some(spot) => {
                lines.push(best_spot::spot_line(&spot));
                spots.push(spot);
            }
            // The window of this spot is too short: ask for a moment more.
            None => lines.push(best_spot::hold_line().to_string()),
        }
    }
    lines.push(best_spot::verdict(&spots));
    lines.join("\n")
}

/// The best-spot lines of the phone screen.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_bestSpotLines(
    mut env: JNIEnv,
    _class: JClass,
    samples: JString,
    marks: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let samples = jni_string(&mut env, &samples);
        let marks = jni_string(&mut env, &marks);
        let text = best_spot_lines(&samples, &marks);
        jni_string_out(&mut env, &text)
    })
}

/// The length of a best-spot session in milliseconds. The core owns the
/// number, so the phone cannot decide a session length of its own.
fn best_spot_session_ms() -> i64 {
    (jafnify_core::best_spot::SESSION_SECS as i64) * 1000
}

/// The length of a best-spot session in milliseconds, from the core.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_bestSpotSessionMs(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    jni_guard(0, best_spot_session_ms)
}

/// The device panel lines of the phone screen, in core wording.
fn device_panel_lines(facts: &str) -> String {
    jafnify_core::device_panel::DeviceFacts::parse(facts).text()
}

/// The device panel lines of the phone screen.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_devicePanel(
    mut env: JNIEnv,
    _class: JClass,
    facts: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let facts = jni_string(&mut env, &facts);
        let text = device_panel_lines(&facts);
        jni_string_out(&mut env, &text)
    })
}

fn budget_record(
    allow_bytes: u64,
    used_bytes: u64,
    reset_day: i32,
    month_key: u32,
    warned_month: u32,
) -> String {
    jafnify_core::budget::BudgetRecord {
        allow_bytes,
        used_bytes,
        reset_day: reset_day.clamp(0, 31) as u8,
        month_key,
        warned_month,
    }
    .line()
}

fn budget_display(record_line: &str) -> String {
    jafnify_core::budget::BudgetRecord::parse_line(record_line)
        .map(|record| record.display_line())
        .unwrap_or_default()
}

fn budget_warning(record_line: &str) -> String {
    jafnify_core::budget::BudgetRecord::parse_line(record_line)
        .ok()
        .and_then(|record| record.warning_line())
        .unwrap_or("")
        .to_string()
}

fn battery_lines(level_pct: i32, temp_c_x10: i32) -> String {
    let level = (0..=100).contains(&level_pct).then_some(level_pct as u8);
    let temp = i16::try_from(temp_c_x10)
        .ok()
        .filter(|value| *value != i16::MIN);
    jafnify_core::battery::lines(level, temp).join("\n")
}

/// The reason the phone's previous process ended, from Android's
/// `ApplicationExitInfo` or from the package's own install time: `reason` is
/// `getReason()` and `age_ms` how long ago the process died. The core decides
/// whether it belongs to this session. The provider keeps it for the next
/// answers: the local sampler rewrites the line every second, while the
/// laptop asks every five, so a single line would almost always miss it.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_setExitReason(
    _env: JNIEnv,
    _class: JClass,
    reason: jint,
    age_ms: jlong,
) {
    jni_guard((), || {
        if let Some(reason) = jafnify_core::phone_exit::from_android(reason, age_ms) {
            joiner::phone_stats_provider().set_exit(reason);
        }
    })
}

/// The summary line for one sampler tick; a negative counter is zero.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_phoneStatsLine(
    mut env: JNIEnv,
    _class: JClass,
    up_bps: jlong,
    down_bps: jlong,
    network: JString,
    signal: jint,
    dbm: jint,
    battery_pct: jint,
    temp_c_x10: jint,
    used_bytes: jlong,
    allow_bytes: jlong,
    reset_day: jint,
    month_key: jint,
    switches: jint,
    with_dns: jboolean,
    thermal_status: jint,
    thermal_headroom_pct: jint,
    network_permission: jboolean,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let network = jni_string(&mut env, &network);
        let rtt_ms = if with_dns != 0 {
            libwgslirpy::router::dns_answer_median_ms()
        } else {
            None
        };
        let line = phone_stats_line_with_exit(
            u64::try_from(up_bps).unwrap_or(0),
            u64::try_from(down_bps).unwrap_or(0),
            &network,
            signal,
            dbm,
            battery_pct,
            temp_c_x10,
            u64::try_from(used_bytes).unwrap_or(0),
            u64::try_from(allow_bytes).unwrap_or(0),
            reset_day,
            u32::try_from(month_key).unwrap_or(0),
            switches,
            rtt_ms,
            thermal_status,
            thermal_headroom_pct,
            network_permission != 0,
            None,
        );
        jni_string_out(&mut env, &line)
    })
}

/// One budget record line; the phone stores this exact text.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_budgetRecord(
    mut env: JNIEnv,
    _class: JClass,
    allow_bytes: jlong,
    used_bytes: jlong,
    reset_day: jint,
    month_key: jint,
    warned_month: jint,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let line = budget_record(
            u64::try_from(allow_bytes).unwrap_or(0),
            u64::try_from(used_bytes).unwrap_or(0),
            reset_day,
            u32::try_from(month_key).unwrap_or(0),
            u32::try_from(warned_month).unwrap_or(0),
        );
        jni_string_out(&mut env, &line)
    })
}

/// The read-only budget line from a stored record; "" when it does not parse.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_budgetDisplay(
    mut env: JNIEnv,
    _class: JClass,
    record: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let record = jni_string(&mut env, &record);
        jni_string_out(&mut env, &budget_display(&record))
    })
}

/// The fixed 80 percent warning line, or "" when it is not due.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_budgetWarning(
    mut env: JNIEnv,
    _class: JClass,
    record: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let record = jni_string(&mut env, &record);
        jni_string_out(&mut env, &budget_warning(&record))
    })
}

/// The battery lines for a level and a temperature; unknown values are skipped.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_batteryLines(
    mut env: JNIEnv,
    _class: JClass,
    level_pct: jint,
    temp_c_x10: jint,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        jni_string_out(&mut env, &battery_lines(level_pct, temp_c_x10))
    })
}

static USAGE_SAMPLER: OnceLock<Mutex<jafnify_core::usage::UsageSampler>> = OnceLock::new();

fn usage_sampler() -> &'static Mutex<jafnify_core::usage::UsageSampler> {
    USAGE_SAMPLER.get_or_init(|| Mutex::new(jafnify_core::usage::UsageSampler::new()))
}

fn usage_step(
    at_ms: i64,
    up_bytes: u64,
    down_bytes: u64,
    connected: bool,
    network: &str,
) -> String {
    let mut sampler = usage_sampler().lock().unwrap_or_else(|e| e.into_inner());
    sampler
        .step(jafnify_core::usage::UsageSample {
            at_ms,
            up_bytes,
            down_bytes,
            connected,
            network: jafnify_core::phone_stats::NetworkKind::parse(network),
        })
        .map(|bucket| bucket.line())
        .unwrap_or_default()
}

fn usage_summary(lines: &str, window_days: i32, now_ms: i64, offset_minutes: i32) -> String {
    let buckets: Vec<jafnify_core::usage::UsageBucket> = lines
        .lines()
        .filter_map(|line| jafnify_core::usage::UsageBucket::parse_line(line.trim()).ok())
        .collect();
    let window = match window_days {
        1 => jafnify_core::usage::UsageWindow::Today,
        7 => jafnify_core::usage::UsageWindow::SevenDays,
        _ => jafnify_core::usage::UsageWindow::ThirtyDays,
    };
    jafnify_core::usage::UsageSummary::build(&buckets, window, now_ms, offset_minutes)
        .lines()
        .join("\n")
}

/// The six core summary lines for one window, from the caller's stored lines.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_usageSummary(
    mut env: JNIEnv,
    _class: JClass,
    lines: JString,
    window_days: jint,
    now_ms: jlong,
    offset_minutes: jint,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let lines = jni_string(&mut env, &lines);
        jni_string_out(
            &mut env,
            &usage_summary(&lines, window_days, now_ms, offset_minutes),
        )
    })
}

/// The connection details lines for the phone, in the core's words.
fn connection_details_lines(health_line: &str, endpoint: &str) -> String {
    let path = match health_line {
        "Direct over hotspot" | "Waiting for the first handshake" | "Not reachable" => {
            Some(health_line.to_string())
        }
        _ => None,
    };
    let details = jafnify_core::connection_details::ConnectionDetails {
        path,
        verdict: (!health_line.is_empty()).then(|| health_line.to_string()),
        dns: jafnify_core::connection_details::DnsMode::ThroughTunnel,
        endpoint: (!endpoint.is_empty()).then(|| endpoint.to_string()),
    };
    details.text_lines(jafnify_core::connection_details::DeviceSide::Phone)
}

/// The connection details of the phone, one readable line per row.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_connectionDetails(
    mut env: JNIEnv,
    _class: JClass,
    health_line: JString,
    endpoint: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let health_line = jni_string(&mut env, &health_line);
        let endpoint = jni_string(&mut env, &endpoint);
        let text = connection_details_lines(&health_line, &endpoint);
        jni_string_out(&mut env, &text)
    })
}

/// The wire word of a modem kind; empty when the kind is unknown.
fn generation_word(kind: &str) -> String {
    jafnify_core::network_report::Generation::from_kind(kind)
        .map(|generation| generation.wire().to_string())
        .unwrap_or_default()
}

/// True when the reported generation changed between two known kinds.
fn generation_switch(previous: &str, current: &str) -> bool {
    match (
        jafnify_core::network_report::Generation::from_kind(previous),
        jafnify_core::network_report::Generation::from_kind(current),
    ) {
        (Some(before), Some(after)) => before != after,
        _ => false,
    }
}

/// The canonical JFN1 line for the given fields; empty when they do not parse.
fn network_bucket_line(fields: &str) -> String {
    let line = format!("JFN1 1 {fields}");
    match jafnify_core::network_report::NetworkBucket::parse_line(&line) {
        Ok(bucket) => bucket.line(),
        Err(_) => String::new(),
    }
}

/// One signal sample line; empty when the level is unknown or too soon.
fn signal_sample(at_ms: i64, level: i32, last_at_ms: i64) -> String {
    if !(0..=4).contains(&level) {
        return String::new();
    }
    if last_at_ms > 0 && at_ms - last_at_ms < jafnify_core::signal_history::INTERVAL_MS {
        return String::new();
    }
    jafnify_core::signal_history::SignalSample {
        at_ms,
        level: level as u8,
    }
    .line()
}

/// The phone's network report: the current hour, the last 24 hours and the
/// signal line, all in the core's wording.
fn network_report(
    network_lines: &str,
    signal_lines: &str,
    now_ms: i64,
    offset_minutes: i32,
) -> String {
    let buckets: Vec<jafnify_core::network_report::NetworkBucket> = network_lines
        .lines()
        .filter_map(|line| {
            jafnify_core::network_report::NetworkBucket::parse_line(line.trim()).ok()
        })
        .collect();
    let mut out = String::new();
    out.push_str("This hour\n");
    match buckets
        .last()
        .filter(|bucket| bucket.hour_start_ms == jafnify_core::usage::hour_start_ms(now_ms))
    {
        Some(bucket) => {
            for line in jafnify_core::network_report::NetworkSummary::build(bucket).lines() {
                out.push_str(&line);
                out.push('\n');
            }
        }
        None => out.push_str("Not enough data yet.\n"),
    }
    out.push_str("\nLast 24 hours\n");
    for line in jafnify_core::network_report::window_lines(&buckets, now_ms) {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("\nSignal\n");
    let mut history = jafnify_core::signal_history::SignalHistory::new();
    for line in signal_lines.lines() {
        if let Some(sample) = jafnify_core::signal_history::SignalSample::parse_line(line.trim()) {
            history.add(sample);
        }
    }
    out.push_str(&history.summary_line(now_ms, offset_minutes));
    out
}

/// The stored same-hour bucket as numbers for the Java sampler:
/// [hour, switches, up, down, drops, connected_ms, samples, drain_mah(-1),
/// drain_pct(-1), charging, count, (ordinal, ms)...].
fn network_seed(line: &str) -> Vec<i64> {
    let Ok(bucket) = jafnify_core::network_report::NetworkBucket::parse_line(line.trim()) else {
        return Vec::new();
    };
    let mut values = vec![
        bucket.hour_start_ms,
        bucket.switches as i64,
        bucket.up_bytes as i64,
        bucket.down_bytes as i64,
        bucket.drops as i64,
        bucket.connected_ms as i64,
        bucket.samples as i64,
        bucket.drain_mah.map(|value| value as i64).unwrap_or(-1),
        bucket.drain_pct.map(|value| value as i64).unwrap_or(-1),
        bucket.charging as i64,
        bucket.generation_ms.len() as i64,
    ];
    for (generation, ms) in &bucket.generation_ms {
        values.push(generation_ordinal(*generation));
        values.push(*ms as i64);
    }
    values
}

fn generation_ordinal(generation: jafnify_core::network_report::Generation) -> i64 {
    use jafnify_core::network_report::Generation::*;
    match generation {
        Lte => 0,
        LteCa => 1,
        LteAdvancedPro => 2,
        Nr => 3,
        NrNsa => 4,
        NrNsaMmwave => 5,
        NrAdvanced => 6,
    }
}

/// Continue a stored hour after a restart (network counters).
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_networkSeed(
    mut env: JNIEnv,
    _class: JClass,
    line: JString,
) -> jlongArray {
    jni_guard(std::ptr::null_mut(), || {
        let line = jni_string(&mut env, &line);
        let values = network_seed(&line);
        // A failed allocation is null, which the Java side treats as
        // "nothing to seed".
        let Ok(array) = env.new_long_array(values.len() as i32) else {
            let _ = env.exception_clear();
            return std::ptr::null_mut();
        };
        if env.set_long_array_region(&array, 0, &values).is_err() {
            let _ = env.exception_clear();
            return std::ptr::null_mut();
        }
        array.into_raw()
    })
}

/// Continue a stored hour after a restart (usage counters).
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_usageSeed(
    mut env: JNIEnv,
    _class: JClass,
    line: JString,
) {
    jni_guard((), || {
        let line = jni_string(&mut env, &line);
        let Ok(bucket) = jafnify_core::usage::UsageBucket::parse_line(line.trim()) else {
            return;
        };
        let mut sampler = usage_sampler().lock().unwrap_or_else(|e| e.into_inner());
        *sampler = jafnify_core::usage::UsageSampler::new();
        sampler.seed(bucket);
    })
}

/// The charge counter in mAh when it is sane; -1 when it is not.
fn sane_mah(counter: i64) -> i64 {
    match jafnify_core::network_report::sane_mah(counter) {
        Some(value) => value as i64,
        None => -1,
    }
}

/// The phone's network report as readable lines; never null.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_networkReport(
    mut env: JNIEnv,
    _class: JClass,
    network_lines: JString,
    signal_lines: JString,
    now_ms: jlong,
    offset_minutes: jint,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let network_lines = jni_string(&mut env, &network_lines);
        let signal_lines = jni_string(&mut env, &signal_lines);
        let text = network_report(&network_lines, &signal_lines, now_ms, offset_minutes);
        jni_string_out(&mut env, &text)
    })
}

/// The wire word of the phone's current generation; "" when it is unknown.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_generationWord(
    mut env: JNIEnv,
    _class: JClass,
    kind: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let kind = jni_string(&mut env, &kind);
        jni_string_out(&mut env, &generation_word(&kind))
    })
}

/// True when the generation changed between two kinds.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_generationSwitch(
    mut env: JNIEnv,
    _class: JClass,
    previous: JString,
    current: JString,
) -> jboolean {
    jni_guard(0, || {
        let previous = jni_string(&mut env, &previous);
        let current = jni_string(&mut env, &current);
        u8::from(generation_switch(&previous, &current))
    })
}

/// The canonical JFN1 line for the given fields; "" when they do not parse.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_networkBucketLine(
    mut env: JNIEnv,
    _class: JClass,
    fields: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let fields = jni_string(&mut env, &fields);
        jni_string_out(&mut env, &network_bucket_line(&fields))
    })
}

/// One signal sample line; "" when the level is unknown or the gate says no.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_signalSample(
    mut env: JNIEnv,
    _class: JClass,
    at_ms: jlong,
    level: jint,
    last_at_ms: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        jni_string_out(&mut env, &signal_sample(at_ms, level, last_at_ms))
    })
}

/// The charge counter in mAh when it is sane; -1 when it is not.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_saneMah(
    _env: JNIEnv,
    _class: JClass,
    counter: jlong,
) -> jlong {
    jni_guard(-1, || sane_mah(counter))
}

fn usage_current() -> String {
    usage_sampler()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .current()
        .map(|bucket| bucket.line())
        .unwrap_or_default()
}

fn usage_reset() {
    *usage_sampler().lock().unwrap_or_else(|e| e.into_inner()) =
        jafnify_core::usage::UsageSampler::new();
}

/// Feed one usage sample; a completed hour bucket comes back as its line.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_usageStep(
    mut env: JNIEnv,
    _class: JClass,
    at_ms: jlong,
    up_bytes: jlong,
    down_bytes: jlong,
    connected: jboolean,
    network: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let network = jni_string(&mut env, &network);
        let line = usage_step(
            at_ms,
            u64::try_from(up_bytes).unwrap_or(0),
            u64::try_from(down_bytes).unwrap_or(0),
            connected != 0,
            &network,
        );
        jni_string_out(&mut env, &line)
    })
}

/// The current hour's usage line, or "" before the first sample.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_usageCurrent(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        jni_string_out(&mut env, &usage_current())
    })
}

/// Drop the in-memory usage sampler; the next ON period starts a baseline.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_usageReset(_env: JNIEnv, _class: JClass) {
    jni_guard((), usage_reset)
}

/// Feed or clear (null/empty) the line the tunnel's stats responder answers.
/// The same line feeds the ring of the phone's own series; an empty line means
/// the sharing stopped, so the ring is forgotten.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_setPhoneStats(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
    line: JString,
) {
    jni_guard((), || {
        let line = jni_string(&mut env, &line);
        joiner::phone_stats_provider().set_line(if line.is_empty() {
            None
        } else {
            Some(line.clone())
        });
        with_app(instance, |app| {
            let mut trends = app.trends.lock().unwrap_or_else(|err| err.into_inner());
            push_phone_line(&mut trends, &line, now_ms());
        });
    })
}

/// Fills the ring of the phone's own series with a fresh summary line. An
/// empty line means the sharing stopped: the ring forgets the old numbers.
fn push_phone_line(trends: &mut jafnify_core::tiles::Trends, line: &str, at_ms: i64) {
    if line.trim().is_empty() {
        trends.clear();
        return;
    }
    if let Ok(stats) = jafnify_core::phone_stats::PhoneStats::parse_response(line.trim().as_bytes())
    {
        trends.push(at_ms, &stats, None);
    }
}

/// The line under the pairing code: "" when no code is on screen, "Code
/// expired" once its lifetime is over, else the remaining whole minutes and
/// `m:ss` only in the last minute.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerCodeExpiry(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = with_app(instance, |app| app.joiner.code_expiry()).unwrap_or_default();
        jni_string_out(&mut env, &text)
    })
}

/// Whether the code on screen has expired. The UI shows the recovery action
/// from this fact, so the wording of the line above can change freely.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerCodeExpired(
    _env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jboolean {
    jni_guard(0, || {
        with_app(instance, |app| jboolean::from(app.joiner.code_expired())).unwrap_or(0)
    })
}

/// The health verdict under the status: "" when there is nothing to say.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerHealthLine(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = with_app(instance, |app| app.joiner.health_line()).unwrap_or_default();
        jni_string_out(&mut env, &text)
    })
}

/// The handshake age for the Details view: "" when there is none.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerHealthDetail(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = with_app(instance, |app| app.joiner.health_detail()).unwrap_or_default();
        jni_string_out(&mut env, &text)
    })
}

/// The retry line while the tunnel is retrying: "" otherwise.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerRetryLine(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = with_app(instance, |app| app.joiner.retry_line()).unwrap_or_default();
        jni_string_out(&mut env, &text)
    })
}

/// The text the Java journal writes. The on-disk journal is local debugging
/// material, so it keeps IP addresses and ports; only an export redacts.
fn journal_diag_text(diag: String) -> String {
    diag
}

/// Additive diagnostic snapshot for the journal: one `STATS` line with
/// counters/addresses, then zero or more `EVENT` lines. Values are
/// whitespace-free; no code, key or packet payload is included.
///
/// The journal keeps addresses for local debugging; the export path
/// (`redactForExport`) redacts a copy.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_sharerDiag(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let diag =
            with_app(instance, |app| journal_diag_text(app.joiner.diag())).unwrap_or_default();
        jni_string_out(&mut env, &diag)
    })
}

/// Redact an export built in Java (the connection log and the Java half of
/// the journal). `aliases` has one peer per line; the spellings of that peer
/// are separated by tabs. Each peer is registered in line order, so its label
/// is `peer-1`, `peer-2`, ... and is the same in every piece of the export.
/// The whole text goes through one redactor, so one peer keeps one label
/// across every line.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_redactForExport(
    mut env: JNIEnv,
    _class: JClass,
    text: JString,
    aliases: JString,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = jni_string(&mut env, &text);
        let aliases = jni_string(&mut env, &aliases);
        let lines: Vec<Vec<&str>> = aliases
            .lines()
            .map(|line| {
                line.split('\t')
                    .map(str::trim)
                    .filter(|alias| !alias.is_empty())
                    .collect::<Vec<&str>>()
            })
            .filter(|peer| !peer.is_empty())
            .collect();
        let peers: Vec<&[&str]> = lines.iter().map(|peer| peer.as_slice()).collect();
        let redacted = jafnify_core::redact::redact_export_with(&text, &peers, "");
        jni_string_out(&mut env, &redacted)
    })
}

/// Debug probe: discovery and the 0x02 key exchange on the listed interfaces
/// ("name=ip/prefix,..."), no tunnel, UDP 9797 never bound. Returns the code
/// the laptop must use (`jafnify probe --code`), or "error: ..." on failure.
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_probeStart(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
    ifaces: JString,
    seconds: jint,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let ifaces = jni_string(&mut env, &ifaces);
        let ifaces = jafnify_core::probe::parse_local_ifaces(&ifaces);
        let out = with_app(instance, |app| {
            match app.probe.start(ifaces, seconds.max(1) as u64) {
                Ok(code) => code,
                Err(e) => format!("error: {e}"),
            }
        })
        .unwrap_or_else(|| "error: no instance".to_string());
        jni_string_out(&mut env, &out)
    })
}

/// The probe report so far, one line each ("" before any start).
#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_probeStatus(
    mut env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jstring {
    jni_guard(std::ptr::null_mut(), || {
        let text = with_app(instance, |app| app.probe.status()).unwrap_or_default();
        jni_string_out(&mut env, &text)
    })
}

#[no_mangle]
pub extern "system" fn Java_com_siliconstencil_jafnify_Native_probeRunning(
    _env: JNIEnv,
    _class: JClass,
    instance: jlong,
) -> jni::sys::jboolean {
    jni_guard(0, || {
        with_app(instance, |app| app.probe.running() as jni::sys::jboolean).unwrap_or(0)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_network_seed_vector_matches_the_java_unpacking() {
        // NetworkSampler.java reads [hour, switches, up, down, drops,
        // connected_ms, samples, mah|-1, pct|-1, charging, count, (ordinal, ms)*]
        // and maps ordinals to {"4g","4g-ca","4g+","5g","5g-nsa","5g-mmwave","5g-adv"}.
        let line = "JFN1 1 hour=3600000 switches=3 gen=5g-nsa:600000,4g+:300000 up=10 down=20 drops=1 batt_mah=40 charging=0 connected_ms=900000 samples=15";
        let v = network_seed(line);
        assert_eq!(
            &v[..11],
            &[3_600_000, 3, 10, 20, 1, 900_000, 15, 40, -1, 0, 2]
        );
        let words = ["4g", "4g-ca", "4g+", "5g", "5g-nsa", "5g-mmwave", "5g-adv"];
        let mut pairs: Vec<(&str, i64)> = v[11..]
            .chunks(2)
            .map(|pair| (words[pair[0] as usize], pair[1]))
            .collect();
        pairs.sort();
        assert_eq!(pairs, vec![("4g+", 300_000), ("5g-nsa", 600_000)]);
        assert!(network_seed("garbage").is_empty());
    }

    use super::*;
    use jafnify_core::flags::{platform, Bucket, FeatureFlag};

    // F1: a panic inside a JNI body must come back as the safe default, not
    // unwind across `extern "system"` and abort the app.
    #[test]
    fn a_panicking_jni_body_returns_the_safe_default() {
        assert_eq!(jni_guard(-1, || panic!("injected JNI panic")), -1);
        assert_eq!(jni_guard(0i64, || 7), 7);
    }

    // F2: a zero or stale handle is rejected, never dereferenced; destroy is
    // a no-op the second time.
    #[test]
    fn a_stale_or_zero_instance_handle_is_rejected_and_destroy_is_idempotent() {
        assert!(with_app(0, |_| true).is_none());
        assert!(with_app(-7, |_| true).is_none());
        let handle = create_app();
        assert!(handle > 0);
        assert!(with_app(handle, |_| true).is_some());
        destroy_app(handle);
        assert!(with_app(handle, |_| true).is_none());
        // A second destroy must not free anything again.
        destroy_app(handle);
        assert!(with_app(handle, |_| true).is_none());
        assert!(with_app(9_999_999, |_| true).is_none());
    }

    // A slow call on one thread must not block another thread's call on the
    // same instance: the registry lock is not held during the call.
    #[test]
    fn a_slow_jni_call_does_not_block_another_thread() {
        let handle = create_app();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let slow = std::thread::spawn(move || {
            with_app(handle, |_| {
                started_tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(800));
            })
        });
        started_rx.recv().unwrap();
        let begin = std::time::Instant::now();
        assert!(with_app(handle, |_| true).is_some());
        assert!(begin.elapsed() < std::time::Duration::from_millis(300));
        // A destroy while the slow call runs only drops the registry's
        // reference; the running call keeps the instance alive.
        destroy_app(handle);
        assert!(slow.join().unwrap().is_some());
        assert!(with_app(handle, |_| true).is_none());
    }

    #[test]
    fn the_journal_path_keeps_the_raw_public_endpoint() {
        let raw = "STATS peer=203.0.113.5:9797 local=10.1.0.2";
        assert_eq!(journal_diag_text(raw.to_string()), raw);
    }

    fn synthetic(
        id: &'static str,
        bucket: Bucket,
        default: bool,
        platforms: u8,
        wired: bool,
    ) -> FeatureFlag {
        FeatureFlag {
            id,
            bucket,
            default,
            platforms,
            label: "Synthetic row",
            help: "Synthetic help.",
            wired,
        }
    }

    #[test]
    fn a_flag_line_has_the_fixed_six_fields() {
        let line = flag_line(&synthetic(
            "synthetic",
            Bucket::Settings,
            false,
            platform::ANDROID,
            true,
        ));
        assert_eq!(
            line,
            "synthetic\tsettings\tfalse\t1\tSynthetic row\tSynthetic help."
        );
    }

    #[test]
    fn flags_meta_keeps_only_wired_rows_for_the_platform() {
        let table = [
            synthetic(
                "wired_android",
                Bucket::Advanced,
                true,
                platform::ANDROID,
                true,
            ),
            synthetic("unwired", Bucket::Settings, false, platform::ANDROID, false),
            synthetic(
                "linux_only",
                Bucket::Experimental,
                false,
                platform::LINUX,
                true,
            ),
        ];
        assert_eq!(
            flags_meta_text(&table, platform::ANDROID),
            "wired_android\tadvanced\ttrue\t1\tSynthetic row\tSynthetic help."
        );
    }

    #[test]
    fn a_network_report_uses_the_core_wording() {
        let net = "JFN1 1 hour=3600000 switches=4 gen=5g:3600000 up=1000 down=2000 drops=1 batt_mah=40 charging=0 connected_ms=3600000 samples=60";
        let sig = "JSG1 1 at=3400000 level=1\nJSG1 1 at=3599000 level=3";
        let out = network_report(net, sig, 3_600_000, 0);
        assert!(out.contains("Network switches this hour: 4"), "{out}");
        assert!(
            out.contains("Network stability this hour: switching often."),
            "{out}"
        );
        assert!(
            out.contains("Signal: 3/4 now; weakest 1/4 at 00:56."),
            "{out}"
        );
        assert!(out.contains("Last 24 hours"), "{out}");
        assert!(out.contains("Network switches: 4"), "{out}");
        assert!(!out.to_lowercase().contains("packet loss"), "{out}");
        assert!(!out.contains("caused"), "{out}");
    }

    #[test]
    fn a_phone_stats_line_has_the_core_format() {
        assert_eq!(
            phone_stats_line(1234, 5678, "wifi", -1, i32::MIN, 80, 315, 0, 0, 0, 0, -1, None, -1),
            "JFS1 1 summary v=1 net=wifi sig=255 rtt=0 up=1234 down=5678 batt=80 temp=315 used=0 allow=0 reset=0 month=0"
        );
        assert_eq!(
            phone_stats_line(0, 0, "cellular", -1, i32::MIN, 255, i32::MIN, 0, 0, 0, 0, -1, None, -1),
            "JFS1 1 summary v=1 net=mobile sig=255 rtt=0 up=0 down=0 batt=255 temp=-32768 used=0 allow=0 reset=0 month=0"
        );
    }

    #[test]
    fn a_phone_stats_line_carries_the_signal_strength() {
        let line = phone_stats_line(0, 0, "5g-adv", 3, -95, 80, 315, 0, 0, 1, 24323, 4, None, -1);
        assert!(line.contains(" dbm=-95"), "{line}");
        let unknown = phone_stats_line(
            0,
            0,
            "5g-adv",
            3,
            i32::MIN,
            80,
            315,
            0,
            0,
            1,
            24323,
            4,
            None,
            -1,
        );
        assert!(!unknown.contains("dbm="), "{unknown}");
        let bad = phone_stats_line(
            0, 0, "5g-adv", 3, -500, 80, 315, 0, 0, 1, 24323, 4, None, -1,
        );
        assert!(!bad.contains("dbm="), "{bad}");
        // The modem's unknown value (CellInfo.UNAVAILABLE) is Integer.MAX_VALUE:
        // it never travels as a number either.
        let unavailable = phone_stats_line(
            0,
            0,
            "5g-adv",
            3,
            i32::MAX,
            80,
            315,
            0,
            0,
            1,
            24323,
            4,
            None,
            -1,
        );
        assert!(!unavailable.contains("dbm="), "{unavailable}");
    }

    #[test]
    fn a_phone_stats_line_carries_the_thermal_status() {
        let line = phone_stats_line(0, 0, "5g-adv", 3, -95, 80, 315, 0, 0, 1, 24323, 4, None, 3);
        assert!(line.contains(" thr=3"), "{line}");
        // A phone below API 29 reports nothing; the key stays out of the line.
        let unknown =
            phone_stats_line(0, 0, "5g-adv", 3, -95, 80, 315, 0, 0, 1, 24323, 4, None, -1);
        assert!(!unknown.contains("thr="), "{unknown}");
        // A value outside the range of Android is not a thermal status.
        let bad = phone_stats_line(0, 0, "5g-adv", 3, -95, 80, 315, 0, 0, 1, 24323, 4, None, 9);
        assert!(!bad.contains("thr="), "{bad}");
    }

    #[test]
    fn the_summary_carries_the_thermal_headroom_forecast() {
        let line = phone_stats_line_with_exit(
            0, 0, "5g-adv", 3, -95, 80, 315, 0, 0, 1, 24323, 4, None, 0, 105, false, None,
        );
        assert!(line.contains(" hr30=105"), "{line}");
        // A negative value means the phone does not support it: no key.
        let absent = phone_stats_line_with_exit(
            0, 0, "5g-adv", 3, -95, 80, 315, 0, 0, 1, 24323, 4, None, 0, -1, false, None,
        );
        assert!(!absent.contains("hr30="), "{absent}");
    }

    #[test]
    fn the_summary_carries_the_exit_reason_once() {
        let base = |exit| {
            phone_stats_line_with_exit(
                0, 0, "5g-adv", 3, -95, 80, 315, 0, 0, 1, 24323, 4, None, -1, -1, false, exit,
            )
        };
        assert!(
            !base(None).contains("exit="),
            "an old phone never sends the key"
        );
        let line = base(Some(jafnify_core::phone_exit::ExitReason::Updated));
        assert!(line.contains(" exit=updated"), "{line}");
        // The core's own parser ignores the key: an older laptop reads the
        // same line without it.
        let parsed = jafnify_core::phone_stats::PhoneStats::parse_response(line.as_bytes());
        assert!(parsed.is_ok(), "{parsed:?}");
    }

    #[test]
    fn the_best_spot_lines_are_the_core_lines() {
        let samples = "1000,5g-adv,3,-95,40,\n\
                       2000,5g-adv,3,-95,40,\n\
                       3000,5g-adv,3,-93,40,\n\
                       4000,5g-adv,3,-93,40,\n\
                       5000,5g-adv,3,-93,40,\n\
                       6000,5g-adv,3,-93,40,";
        let lines = best_spot_lines(samples, "6000,1");
        assert_eq!(
            lines,
            "Now: 5G (advanced), -93 dBm, signal 3/4, DNS 40 ms\n\
             Spot 1: 5G (advanced), -93 dBm, signal 3/4\n\
             Mark another spot to compare"
        );
        // A mark without enough samples in the window scores nothing, so the
        // spot line is the core's hold line.
        let early = best_spot_lines("1000,5g-adv,3,-95,40,", "1000,1");
        assert_eq!(
            early,
            "Now: 5G (advanced), -95 dBm, signal 3/4, DNS 40 ms\n\
             Hold the phone still a few seconds longer\n\
             Mark another spot to compare"
        );
    }

    #[test]
    fn the_session_length_comes_from_the_core() {
        // The phone asks the core how long a session lasts; it never decides
        // a length of its own.
        assert_eq!(
            best_spot_session_ms(),
            (jafnify_core::best_spot::SESSION_SECS as i64) * 1000
        );
        assert_eq!(best_spot_session_ms(), 120_000);
    }

    #[test]
    fn the_device_panel_lines_are_the_core_lines() {
        let text = device_panel_lines(
            "maker=Acme;model=One;android=14;sdk=34;app_version=0.3.0;battery_pct=78;charging=true;temp_c_x10=395;battery_saver=true",
        );
        assert!(text.contains("Phone: Acme One"), "{text}");
        assert!(text.contains("Android: 14 (SDK 34)"), "{text}");
        assert!(text.contains("Jafnify: 0.3.0"), "{text}");
        assert!(text.contains("Battery: 78%, charging"), "{text}");
        assert!(text.contains("Phone temperature: 39.5 C"), "{text}");
        assert!(text.contains("Battery saver: on"), "{text}");
        assert!(
            text.contains("Battery saver is on: Android may stop sharing in the background."),
            "{text}"
        );
        assert_eq!(device_panel_lines(""), "");
    }

    #[test]
    fn a_phone_stats_line_carries_the_dns_answer_time() {
        assert_eq!(
            phone_stats_line(1234, 5678, "wifi", -1, i32::MIN, 80, 315, 0, 0, 0, 0, -1, Some(45), -1),
            "JFS1 1 summary v=1 net=wifi sig=255 rtt=45 up=1234 down=5678 batt=80 temp=315 used=0 allow=0 reset=0 month=0"
        );
        assert_eq!(
            phone_stats_line(1, 2, "wifi", -1, i32::MIN, 80, 315, 0, 0, 0, 0, -1, Some(0), -1),
            "JFS1 1 summary v=1 net=wifi sig=255 rtt=0 up=1 down=2 batt=80 temp=315 used=0 allow=0 reset=0 month=0"
        );
        assert_eq!(
            phone_stats_line(1, 2, "wifi", -1, i32::MIN, 80, 315, 0, 0, 0, 0, -1, Some(60_001), -1),
            "JFS1 1 summary v=1 net=wifi sig=255 rtt=0 up=1 down=2 batt=80 temp=315 used=0 allow=0 reset=0 month=0"
        );
    }

    #[test]
    fn a_phone_section_reads_the_m0_line() {
        let line = phone_stats_line(
            0,
            0,
            "5g-adv",
            3,
            i32::MIN,
            80,
            315,
            0,
            0,
            1,
            24323,
            4,
            Some(45),
            -1,
        );
        assert!(line.contains("sig=3"), "{line}");
        let text = phone_section(&line, true, true, true);
        assert!(text.contains("Network: 5G (advanced)"), "{text}");
        assert!(text.contains("Signal: 3 of 4"), "{text}");
        assert!(text.contains("DNS answer time: 45 ms"), "{text}");
        assert!(text.contains("Network switches this hour: 4"), "{text}");
        // Each line follows its own switch.
        let off = phone_section(&line, false, false, true);
        assert!(!off.contains("DNS"), "{off}");
        assert!(!off.contains("switches"), "{off}");
        assert!(off.contains("Network: 5G (advanced)"), "{off}");
        assert!(phone_section("garbage", true, true, true).is_empty());
    }

    #[test]
    fn the_phone_section_says_when_the_phone_is_hot() {
        let line = phone_stats_line(
            0,
            0,
            "5g-adv",
            4,
            i32::MIN,
            80,
            431,
            0,
            0,
            1,
            24323,
            4,
            Some(45),
            -1,
        );
        let text = phone_section(&line, true, true, true);
        assert!(text.contains("Phone is hot (43.1 C)"), "{text}");
        assert!(
            !text.contains("Phone is hot: 43.1 C."),
            "the phone shows one temperature, not two: {text}"
        );
    }

    #[test]
    fn the_phone_section_hides_the_heat_line_without_the_battery_switch() {
        let line = phone_stats_line(
            0,
            0,
            "5g-adv",
            4,
            i32::MIN,
            80,
            431,
            0,
            0,
            1,
            24323,
            4,
            Some(45),
            -1,
        );
        let text = phone_section(&line, true, true, false);
        assert!(!text.contains("Phone is hot ("), "{text}");
    }

    #[test]
    fn the_phone_section_says_nothing_about_a_warm_phone() {
        let line = phone_stats_line(
            0,
            0,
            "5g-adv",
            4,
            i32::MIN,
            80,
            400,
            0,
            0,
            1,
            24323,
            4,
            Some(45),
            -1,
        );
        let text = phone_section(&line, true, true, true);
        assert!(!text.contains("Phone is hot (40 C)"), "{text}");
        // Below 42 C the battery feature keeps its own temperature line.
        assert!(text.contains("Phone is hot: 40 C."), "{text}");
    }

    #[test]
    fn a_budget_record_has_the_core_format() {
        assert_eq!(
            budget_record(10_000_000_000, 3_000_000_000, 12, 24_323, 24_320),
            "JFB1 1 allow=10000000000 used=3000000000 reset=12 month=24323 warned=24320"
        );
    }

    #[test]
    fn budget_display_and_warning_use_the_core_wording() {
        let record = budget_record(10_000_000_000, 3_200_000_000, 12, 24_323, 0);
        assert_eq!(
            budget_display(&record),
            "Mobile data: 3.2 GB of 10 GB this month."
        );
        assert_eq!(budget_warning(&record), "");
        let high = budget_record(10_000_000_000, 8_000_000_000, 12, 24_323, 0);
        assert_eq!(
            budget_warning(&high),
            "80% of your mobile data allowance used."
        );
        assert_eq!(budget_display("not a record"), "");
    }

    #[test]
    fn the_phone_connection_details_use_the_core_wording() {
        let lines = connection_details_lines("Direct over hotspot", "10.1.0.2:9797");
        assert_eq!(
            lines,
            "Path: Direct over hotspot\n\
             MTU: 1280\n\
             DNS: through the tunnel\n\
             Laptop address: 10.1.0.2:9797"
        );
        let empty = connection_details_lines("", "");
        assert!(empty.contains("Path: unknown"), "{empty}");
        assert!(empty.contains("Laptop address: unknown"), "{empty}");
    }

    #[test]
    fn battery_lines_use_the_core_thresholds() {
        assert_eq!(
            battery_lines(80, 315),
            "Phone battery: 80%.\nPhone temperature: 31.5 C."
        );
        assert_eq!(
            battery_lines(18, 410),
            "Phone battery is low: 18%.\nPhone is hot: 41 C."
        );
        assert_eq!(battery_lines(255, i32::MIN), "");
    }

    #[test]
    fn a_usage_summary_is_built_from_core_lines_in_core_wording() {
        let bucket = jafnify_core::usage::UsageBucket {
            hour_start_ms: 0,
            up_bytes: 100,
            down_bytes: 300,
            min_up_bps: 10,
            min_down_bps: 30,
            peak_up_bps: 50,
            peak_down_bps: 70,
            connected_ms: 1_000,
            samples: 1,
            network: jafnify_core::phone_stats::NetworkKind::Wifi,
        };
        let lines = usage_summary(&bucket.line(), 1, 1_800_000, 0);
        assert_eq!(
            lines,
            "Shared today: 400 B\n\
             Average: 300 B/s down, 100 B/s up\n\
             Peak: 70 B/s down, 50 B/s up\n\
             Lowest: 30 B/s down, 10 B/s up\n\
             Busiest: 00:00-01:00\n\
             Network: Wi-Fi"
        );
        assert!(usage_summary("not a bucket", 7, 0, 0).starts_with("Shared this week: 0 B"));
        assert!(usage_summary("", 30, 0, 0).starts_with("Shared this month: 0 B"));
    }

    #[test]
    fn the_usage_sampler_global_returns_core_lines() {
        usage_reset();
        assert_eq!(usage_step(1_000, 0, 0, true, "wifi"), "");
        assert_eq!(usage_step(2_000, 1_000, 2_000, true, "wifi"), "");
        let current = usage_current();
        assert!(current.starts_with("JFU1 1 hour="), "{current}");
        assert!(current.contains(" up=1000 down=2000 "), "{current}");
        assert!(current.contains(" net=wifi"), "{current}");
    }

    #[test]
    fn todays_table_is_the_android_rows() {
        assert_eq!(
            flags_meta_text(jafnify_core::flags::FLAGS, platform::ANDROID),
            concat!(
                "start_after_restart\tsettings\tfalse\t1\tStart sharing after the phone restarts\tStarts sharing again after the device restarts, if a device is paired.",
                "\nresume_if_stopped\tsettings\tfalse\t1\tResume sharing if the system stops the app\tStarts sharing again after the system stops the app, if a device is paired.",
                "\ndata_budget\tsettings\tfalse\t1\tMobile data budget\tCounts the data this app shares on mobile and warns at 80% of the allowance you enter.",
                "\nkeep_screen_on\tsettings\tfalse\t1\tKeep the screen on while sharing\tKeeps the screen on while Jafnify is on the screen and sharing. It only works while the app is open. It uses more battery and a screen left on warms the phone. Only for phones that stop sharing when the screen turns off.",
                "\nnetwork_details\tsettings\tfalse\t1\tNetwork details\tShows how long the phone was on each network generation, how often it switched, the battery drain of the same hours and the signal history of the day. Needs the phone's network permission.",
                "\nbest_spot\texperimental\tfalse\t3\tFind the best spot for the phone\tShows the signal and the delay every second for two minutes, so you can compare where the phone sits. On the phone the same switch starts the session.",
                "\ndevice_panel\texperimental\tfalse\t1\tShow what the phone can do right now\tShows the phone's battery, memory and the limits Android puts on Jafnify. Nothing is sent anywhere.",
            )
        );
    }

    #[test]
    fn no_standard_row_reaches_the_android_switch_list() {
        let text = flags_meta_text(jafnify_core::flags::FLAGS, platform::ANDROID);
        for id in [
            "tile_home",
            "phone_stats",
            "usage_statistics",
            "battery_status",
        ] {
            assert!(
                !text
                    .lines()
                    .any(|line| line.starts_with(&format!("{id}\t"))),
                "{id} is standard and must not be a switch"
            );
        }
        for id in ["network_report", "signal_history", "test_link"] {
            assert!(
                !text
                    .lines()
                    .any(|line| line.starts_with(&format!("{id}\t"))),
                "{id} is gone"
            );
        }
    }

    #[test]
    fn a_bucket_line_has_the_fixed_four_fields() {
        assert_eq!(bucket_line(Bucket::Settings), "settings\tSettings\t0\t");
        assert_eq!(
            bucket_line(Bucket::Experimental),
            "experimental\tExperimental\t1\tMay change or be removed."
        );
    }

    #[test]
    fn the_bucket_table_lists_all_three_in_core_order() {
        assert_eq!(
            flags_buckets_meta_text(),
            "settings\tSettings\t0\t\nadvanced\tAdvanced\t1\t\nexperimental\tExperimental\t1\tMay change or be removed."
        );
    }

    #[test]
    fn the_generation_word_and_the_switch_rule_come_from_the_core() {
        assert_eq!(generation_word("nr_nsa"), "5g-nsa");
        assert_eq!(generation_word("lte_advanced_pro"), "4g+");
        assert_eq!(generation_word("nr_advanced"), "5g-adv");
        assert_eq!(generation_word("nr"), "5g");
        assert_eq!(generation_word("unknown"), "");
        assert!(generation_switch("nr", "nr_nsa"));
        assert!(generation_switch("nr", "lte"));
        assert!(!generation_switch("nr", "nr"));
        assert!(!generation_switch("unknown", "nr"));
        assert!(!generation_switch("nr", "unknown"));
    }

    #[test]
    fn a_network_bucket_line_is_the_core_line() {
        let line = network_bucket_line(
            "hour=3600000 switches=1 gen=5g:60000 up=10 down=20 drops=0 charging=0 connected_ms=1000 samples=60",
        );
        assert!(
            line.starts_with("JFN1 1 hour=3600000 switches=1 gen=5g:60000"),
            "{line}"
        );
        assert_eq!(network_bucket_line("hour=x"), "");
    }

    #[test]
    fn a_signal_sample_needs_a_known_level_and_the_five_minute_gate() {
        assert_eq!(
            signal_sample(1_700_000_000_000, 3, 0),
            "JSG1 1 at=1700000000000 level=3"
        );
        assert_eq!(signal_sample(1_700_000_000_000, 9, 0), "");
        assert_eq!(signal_sample(1_700_000_000_000, 3, 1_700_000_000_000), "");
        assert_eq!(
            signal_sample(1_700_000_000_000 + 300_000, 3, 1_700_000_000_000),
            "JSG1 1 at=1700000300000 level=3"
        );
    }

    #[test]
    fn the_charge_counter_sanity_rule_comes_from_the_core() {
        assert_eq!(sane_mah(2400), 2400);
        assert_eq!(sane_mah(0), -1);
        assert_eq!(sane_mah(-5), -1);
    }

    #[test]
    fn the_phone_home_carries_the_card_and_the_tiles() {
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        let facts = "charging=1;battery_saver=0;uptime_secs=4324";
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: line,
            usage_lines: "",
            facts_blob: facts,
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: None,
            state: jafnify_core::tiles::PhoneShareState::Sharing,
            state_line: None,
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "",
            speed_scope: "",
            average_scope: "",
            layout: "",
            session: "",
        });
        let first = text.lines().next().expect("a header");
        assert_eq!(first, "JFH1 1");
        assert!(text.contains("title=Sharing with the laptop"), "{text}");
        assert!(text.contains("label=SIGNAL"), "{text}");
        assert!(text.contains("label=DNS"), "{text}");
        assert!(text.contains("label=HEAT"), "{text}");
        assert!(text.contains("label=BATTERY"), "{text}");
        assert!(text.contains("fact=Android: no throttling"), "{text}");
        assert!(text.contains("action=Stop sharing"), "{text}");
        assert!(text.contains("fact=Charging"), "{text}");
    }

    #[test]
    fn the_phone_home_shows_the_sharing_time_and_not_the_phone_time() {
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        // The phone has been on for 4324 s; the sharing has been up for 72 s.
        let facts = "charging=1;battery_saver=0;uptime_secs=4324";
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: line,
            usage_lines: "",
            facts_blob: facts,
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: Some(72),
            state: jafnify_core::tiles::PhoneShareState::Sharing,
            state_line: None,
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "",
            speed_scope: "",
            average_scope: "",
            layout: "",
            session: "",
        });
        assert!(text.contains("sub=Up 0:01:12"), "{text}");
        assert!(!text.contains("1:12:04"), "{text}");
    }

    #[test]
    fn the_ring_of_the_phone_fills_from_the_summary_line() {
        let mut trends = jafnify_core::tiles::Trends::new();
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        push_phone_line(&mut trends, line, 1_767_225_600_000);
        assert_eq!(trends.dns().len(), 1);
        assert_eq!(trends.signal().len(), 1);
        assert_eq!(trends.phone().len(), 1);
        // The rate of the phone is a ring too: it feeds the SPEED tile.
        assert_eq!(trends.speed().len(), 1);
    }

    #[test]
    fn the_ring_of_the_phone_forgets_when_the_sharing_stops() {
        let mut trends = jafnify_core::tiles::Trends::new();
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        push_phone_line(&mut trends, line, 1_767_225_600_000);
        // An empty line means the sharing stopped: the old numbers are gone.
        push_phone_line(&mut trends, "", 1_767_225_601_000);
        assert!(trends.dns().is_empty());
        assert!(trends.signal().is_empty());
        assert!(trends.phone().is_empty());
        assert!(trends.speed().is_empty());
    }

    #[test]
    fn the_instance_zero_has_a_ring_of_its_own() {
        with_trends(0, |ring| {
            assert!(ring.dns().is_empty());
            assert!(ring.signal().is_empty());
            assert!(ring.phone().is_empty());
        });
    }

    #[test]
    fn the_phone_home_carries_the_card_without_a_summary_line() {
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: "",
            usage_lines: "",
            facts_blob: "",
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: None,
            state: jafnify_core::tiles::PhoneShareState::Idle,
            state_line: None,
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "",
            speed_scope: "",
            average_scope: "",
            layout: "",
            session: "",
        });
        // The card does not need the phone's measurements: the screen must say
        // that the phone is not sharing.
        assert!(text.starts_with("JFH1 1"), "{text}");
        assert!(text.contains("title=Not sharing"), "{text}");
        assert!(text.contains("action=Share"), "{text}");
        // Without the summary line there is nothing to measure.
        assert!(!text.contains("label=SIGNAL"), "{text}");
    }

    #[test]
    fn the_phone_home_says_when_the_phone_is_trying() {
        // The service can run while the tunnel is not up. The card must not
        // say that the phone is sharing then.
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: line,
            usage_lines: "",
            facts_blob: "",
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: None,
            state: jafnify_core::tiles::PhoneShareState::Trying,
            state_line: Some("Retrying in 12 s"),
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "",
            speed_scope: "",
            average_scope: "",
            layout: "",
            session: "",
        });
        assert!(text.contains("title=Trying to share"), "{text}");
        assert!(text.contains("tone=wait"), "{text}");
        assert!(text.contains("sub=Retrying in 12 s"), "{text}");
        assert!(!text.contains("Sharing with the laptop"), "{text}");
    }

    #[test]
    fn the_phone_home_carries_the_shared_data_of_today() {
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        let usage = "JFU1 1 hour=1767225600000 up=208000000 down=695000000 min_up=0 min_down=0 peak_up=120000 peak_down=49900000 connected_ms=3600000 samples=60 net=5g-adv";
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: line,
            usage_lines: usage,
            facts_blob: "",
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: None,
            state: jafnify_core::tiles::PhoneShareState::Sharing,
            state_line: None,
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "",
            speed_scope: "",
            average_scope: "",
            layout: "",
            session: "",
        });
        assert!(text.contains("903.0 MB"), "{text}");
        assert!(text.contains("Peak 49.9 MB/s down"), "{text}");
    }

    #[test]
    fn the_phone_home_carries_the_seven_tiles() {
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=1200000 down=49900000 \
                    batt=74 temp=300 used=1200000000 allow=5000000000 reset=1 month=202611 \
                    thr=0 dbm=-71";
        let usage = "JFU1 1 hour=1767225600000 up=208000000 down=695000000 min_up=0 min_down=0 \
                     peak_up=120000 peak_down=49900000 connected_ms=3600000 samples=60 net=5g-adv";
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: line,
            usage_lines: usage,
            facts_blob: "",
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: Some(72),
            state: jafnify_core::tiles::PhoneShareState::Sharing,
            state_line: None,
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "",
            speed_scope: "",
            average_scope: "",
            layout: "",
            session: "",
        });
        assert_eq!(
            text.lines().filter(|line| line.starts_with("t | ")).count(),
            7
        );
        assert!(text.contains("label=SPEED"), "{text}");
        assert!(
            text.contains("id=speed | label=SPEED | badge= | btone=muted | value=49.9 | unit=MB/s"),
            "{text}"
        );
        assert!(
            text.contains("kind=split | down=695000000 | up=208000000"),
            "{text}"
        );
        assert!(text.contains("label=TODAY"), "{text}");
        assert!(text.contains("Plan: 1.2 GB of 5.0 GB this month"), "{text}");
        assert!(text.contains("sub=Up 0:01:12"), "{text}");
    }

    /// The phone remembers the scope of a tile and hands it in; the core
    /// owns the scope and the words.
    #[test]
    fn the_phone_home_carries_the_scope_of_the_tiles() {
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: line,
            usage_lines: "",
            facts_blob: "",
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: None,
            state: jafnify_core::tiles::PhoneShareState::Sharing,
            state_line: None,
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "SESSION",
            speed_scope: "SESSION",
            average_scope: "TODAY",
            layout: "",
            session: "up=208000000 down=695000000 peak=49900000 start=1767225600000",
        });
        assert!(text.contains("label=SESSION"), "{text}");
        assert!(text.contains("Down 695.0 MB, up 208.0 MB"), "{text}");
        assert!(text.contains("Peak 49.9 MB/s down"), "{text}");
    }

    /// Without a remembered scope the tiles keep the scope of today.
    #[test]
    fn the_phone_home_falls_back_to_today_without_a_scope() {
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        let usage = "JFU1 1 hour=1767225600000 up=208000000 down=695000000 min_up=0 min_down=0 peak_up=120000 peak_down=49900000 connected_ms=3600000 samples=60 net=5g-adv";
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: line,
            usage_lines: usage,
            facts_blob: "",
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: None,
            state: jafnify_core::tiles::PhoneShareState::Sharing,
            state_line: None,
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "",
            speed_scope: "",
            average_scope: "",
            layout: "",
            session: "",
        });
        assert!(text.contains("label=TODAY"), "{text}");
        assert!(text.contains("Peak 49.9 MB/s today"), "{text}");
    }

    /// A scope the tile does not have, or a word the core does not know, falls
    /// back to the default of that tile. The core owns the fallback, not the
    /// bridge.
    #[test]
    fn an_unknown_scope_falls_back_to_the_default_of_the_tile() {
        let line = "JFS1 1 summary v=1 net=5g-adv sig=4 rtt=34 up=0 down=0 batt=74 temp=300 used=0 allow=0 reset=1 month=202611 thr=0 dbm=-71";
        let usage = "JFU1 1 hour=1767225600000 up=208000000 down=695000000 min_up=0 min_down=0 peak_up=120000 peak_down=49900000 connected_ms=3600000 samples=60 net=5g-adv";
        let trends = jafnify_core::tiles::Trends::new();
        let text = phone_home_text(&PhoneHomeArgs {
            summary_line: line,
            usage_lines: usage,
            facts_blob: "",
            trends: &trends,
            now_ms: 1_767_225_600_000,
            offset_minutes: 0,
            uptime_secs: None,
            state: jafnify_core::tiles::PhoneShareState::Sharing,
            state_line: None,
            notice: None,
            show_dns: true,
            show_battery: true,
            network_permission: true,
            today_scope: "NONSENSE",
            // The speed tile has no 24-hour scope: the core falls back to the
            // default of the tile, not to a scope the tile cannot show.
            speed_scope: "24 H",
            average_scope: "",
            layout: "",
            session: "",
        });
        assert!(text.contains("label=TODAY"), "{text}");
        assert!(text.contains("Peak 49.9 MB/s today"), "{text}");
        assert!(!text.contains("in 24 h"), "{text}");
    }

    /// The cycle of the scopes belongs to the core: the phone only asks for
    /// the next scope and stores it.
    #[test]
    fn the_next_scope_of_a_tile_comes_from_the_core() {
        let today = jafnify_core::scope::TileScope::for_tile("today", Some("TODAY"));
        assert_eq!(today.next("today").word(), "SESSION");
        assert_eq!(
            jafnify_core::scope::TileScope::for_tile("today", Some("SESSION"))
                .next("today")
                .word(),
            "TODAY"
        );
        let drops = jafnify_core::scope::TileScope::for_tile("drops", Some("24 H"));
        assert_eq!(drops.next("drops").word(), "SESSION");
    }

    /// The Java declaration and the native function must have the same number
    /// of parameters: the phone and the core call the same function.
    #[test]
    fn the_java_declaration_and_the_native_function_have_the_same_arity() {
        // The type check fails to compile when the native side drifts.
        let _: extern "system" fn(
            JNIEnv,
            JClass,
            jlong,
            JString,
            JString,
            JString,
            JString,
            JString,
            jlong,
            jint,
            jlong,
            jint,
            jboolean,
            jboolean,
            JString,
            JString,
            JString,
            JString,
            JString,
            jboolean,
        ) -> jstring = Java_com_siliconstencil_jafnify_Native_phoneHome;

        let source =
            std::fs::read_to_string("../app/src/main/java/com/siliconstencil/jafnify/Native.java")
                .expect("the Java declaration is there");
        let start = source
            .find("public static native String phoneHome(")
            .expect("the phoneHome declaration");
        let after = &source[start..];
        let open = after.find('(').expect("the parameters");
        let close = after.find(')').expect("the end of the parameters");
        let params = &after[open + 1..close];
        let count = params
            .split(',')
            .filter(|part| !part.trim().is_empty())
            .count();
        assert_eq!(count, 18, "the Java declaration has {count} parameters");
    }

    #[test]
    fn the_phone_stats_line_declaration_and_native_function_have_the_same_arity() {
        let _: extern "system" fn(
            JNIEnv,
            JClass,
            jlong,
            jlong,
            JString,
            jint,
            jint,
            jint,
            jint,
            jlong,
            jlong,
            jint,
            jint,
            jint,
            jboolean,
            jint,
            jint,
            jboolean,
        ) -> jstring = Java_com_siliconstencil_jafnify_Native_phoneStatsLine;

        let source =
            std::fs::read_to_string("../app/src/main/java/com/siliconstencil/jafnify/Native.java")
                .expect("the Java declaration is there");
        let start = source
            .find("public static native String phoneStatsLine(")
            .expect("the phoneStatsLine declaration");
        let after = &source[start..];
        let open = after.find('(').expect("the parameters");
        let close = after.find(')').expect("the end of the parameters");
        let params = &after[open + 1..close];
        let count = params
            .split(',')
            .filter(|part| !part.trim().is_empty())
            .count();
        assert_eq!(count, 16, "the Java declaration has {count} parameters");
    }
}
