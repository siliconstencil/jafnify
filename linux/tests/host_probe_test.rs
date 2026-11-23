//! The host probe: the tools run off the caller's thread, the result is
//! cached for a minute, and a missing or slow tool means no hint.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jafnify_linux::host_hints::HostHint;
use jafnify_linux::host_probe::{CommandRunner, HostProbe};

struct FakeRunner {
    calls: AtomicUsize,
    delay: Duration,
    power_save: Option<String>,
    access_points: bool,
}

impl FakeRunner {
    fn new(power_save: Option<&str>, access_points: bool) -> FakeRunner {
        FakeRunner {
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(0),
            power_save: power_save.map(str::to_string),
            access_points,
        }
    }

    fn shared(self) -> Arc<FakeRunner> {
        Arc::new(self)
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Option<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay.min(timeout));
            return None;
        }
        let joined = format!("{program} {}", args.join(" "));
        if joined == "nmcli -t -f DEVICE,TYPE connection show --active" {
            return self
                .power_save
                .as_ref()
                .map(|iface| format!("{iface}:802-11-wireless\njafn0:wireguard\n"));
        }
        if program == "iw" && joined.ends_with("get power_save") {
            return Some("Power save: on\n".to_string());
        }
        if joined.contains("wifi list") {
            if self.access_points {
                return Some(
                    "*:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Home\n :11\\:22\\:33\\:44\\:55\\:66:Home\n"
                        .to_string(),
                );
            }
            return Some("*:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Home\n".to_string());
        }
        None
    }
}

fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

#[test]
fn the_tools_run_off_the_caller_thread_and_the_hints_arrive() {
    let runner = FakeRunner::new(Some("wlan0"), true).shared();
    let probe = HostProbe::new(runner.clone());

    let started = Instant::now();
    probe.request(None);
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "the request must not wait for the tools"
    );

    let now = Instant::now();
    assert!(wait_for(|| !probe.hints(now).is_empty()));
    let hints = probe.hints(now);
    assert!(hints.contains(&HostHint::WifiPowerSaving {
        iface: "wlan0".to_string()
    }));
    assert!(hints.contains(&HostHint::SeveralAccessPoints));
}

#[test]
fn the_result_is_cached_for_a_minute() {
    let runner = FakeRunner::new(Some("wlan0"), false).shared();
    let probe = HostProbe::new(runner.clone());
    let now = Instant::now();

    probe.request(None);
    assert!(wait_for(|| !probe.hints(now).is_empty()));
    let calls = runner.calls.load(Ordering::SeqCst);

    probe.request(None);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        calls,
        runner.calls.load(Ordering::SeqCst),
        "a second request inside the minute must not run the tools again"
    );
}

#[test]
fn a_missing_tool_means_no_hint() {
    let runner = Arc::new(FakeRunner {
        calls: AtomicUsize::new(0),
        delay: Duration::from_millis(0),
        power_save: None,
        access_points: false,
    });
    let probe = HostProbe::new(runner.clone());
    let now = Instant::now();

    probe.request(None);
    assert!(wait_for(|| runner.calls.load(Ordering::SeqCst) >= 1));
    assert!(probe.hints(now).is_empty());
}

#[test]
fn a_slow_tool_is_dropped_after_the_timeout() {
    let mut slow_runner = FakeRunner::new(Some("wlan0"), false);
    slow_runner.delay = Duration::from_secs(5);
    let runner = slow_runner.shared();
    let probe = HostProbe::new(runner);
    let now = Instant::now();

    probe.request(None);
    std::thread::sleep(Duration::from_millis(300));
    assert!(probe.hints(now).is_empty());
}

#[test]
fn a_cleared_probe_forgets_the_hints() {
    let runner = FakeRunner::new(Some("wlan0"), false).shared();
    let probe = HostProbe::new(runner);
    let now = Instant::now();

    probe.request(None);
    assert!(wait_for(|| !probe.hints(now).is_empty()));
    probe.clear();
    assert!(probe.hints(now).is_empty());
}

#[test]
fn the_probe_never_keeps_the_network_name() {
    let runner = FakeRunner::new(Some("wlan0"), true).shared();
    let probe = HostProbe::new(runner);
    let now = Instant::now();
    probe.request(None);
    assert!(wait_for(|| !probe.hints(now).is_empty()));
    let lines: Vec<String> = probe
        .hints(now)
        .iter()
        .flat_map(|hint| [hint.title(), hint.help()])
        .collect();
    for line in lines {
        assert!(!line.contains("Home"), "{line}");
        assert!(!line.contains("AA:BB"), "{line}");
    }
}

#[test]
fn the_probe_can_be_shared_between_threads() {
    let runner = FakeRunner::new(Some("wlan0"), false).shared();
    let probe = Arc::new(HostProbe::new(runner));
    let worker = probe.clone();
    let handle = std::thread::spawn(move || {
        worker.request(None);
    });
    handle.join().unwrap();
    let now = Instant::now();
    assert!(wait_for(|| !probe.hints(now).is_empty()));
}

#[test]
fn the_cache_is_not_shared_between_probes() {
    let runner = FakeRunner::new(Some("wlan0"), false).shared();
    let probe = HostProbe::new(runner);
    let other = HostProbe::new(FakeRunner::new(None, false).shared());
    let now = Instant::now();
    probe.request(None);
    assert!(wait_for(|| !probe.hints(now).is_empty()));
    assert!(other.hints(now).is_empty());
}
