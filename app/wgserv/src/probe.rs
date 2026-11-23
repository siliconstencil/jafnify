//! Phone-side discovery probe: the core probe on a background thread, with a
//! status text the app can poll. Everything up to tunnel-up (0x01 discovery
//! and the 0x02 key exchange), nothing after: no WireGuard config, no tunnel,
//! and the WireGuard port is never bound.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jafnify_core::pairing::PairingCode;
use jafnify_core::probe::{run_probe, LocalIface, ProbeConfig};

#[derive(Default)]
struct Shared {
    running: bool,
    lines: Vec<String>,
}

/// Poisoned locks are not a reason to abort: the data is plain status text.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One probe at a time. `status()` is the report so far, one line each.
#[derive(Default)]
pub struct ProbeRun {
    shared: Arc<Mutex<Shared>>,
}

/// "A.B.C.D" or "A.B.C.D/P" (the host binary's `--iface-ip`). No prefix means
/// /24; the name is the address (the host knows no interface names).
pub fn parse_iface_arg(arg: &str) -> Option<LocalIface> {
    let (ip, prefix) = match arg.split_once('/') {
        Some((ip, p)) => (ip, p.parse().ok().filter(|p| *p <= 32)?),
        None => (arg, 24),
    };
    Some(LocalIface {
        name: ip.to_string(),
        ip: ip.parse().ok()?,
        prefix,
    })
}

impl ProbeRun {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn running(&self) -> bool {
        lock(&self.shared).running
    }

    /// The report so far (empty before any start).
    pub fn status(&self) -> String {
        lock(&self.shared).lines.join("\n")
    }

    /// Start a probe on `ifaces` for at most `seconds`; returns the code the
    /// other device must use (`jafnify probe --code`). Never falls back to the
    /// default route: with no interface the probe would not mean anything.
    pub fn start(&self, ifaces: Vec<LocalIface>, seconds: u64) -> Result<String, String> {
        self.begin(PairingCode::generate(), ifaces, seconds)
    }

    /// `start` with a code both sides agreed on (the host binary, the lab).
    pub fn start_with_code(
        &self,
        code: &str,
        ifaces: Vec<LocalIface>,
        seconds: u64,
    ) -> Result<String, String> {
        let code = PairingCode::parse(code).ok_or_else(|| "Invalid code".to_string())?;
        self.begin(code, ifaces, seconds)
    }

    fn begin(
        &self,
        code: PairingCode,
        ifaces: Vec<LocalIface>,
        seconds: u64,
    ) -> Result<String, String> {
        if ifaces.is_empty() {
            return Err("no interface to probe on - turn on the hotspot".to_string());
        }
        {
            let mut s = lock(&self.shared);
            if s.running {
                return Err("Already running".to_string());
            }
            s.running = true;
            s.lines = vec!["probe: listening...".to_string()];
        }
        let shared = self.shared.clone();
        let cfg = ProbeConfig {
            code: code.clone(),
            role: jafnify_core::state::Role::Sharer,
            ifaces,
            window: Duration::from_secs(seconds),
            linger: Duration::from_secs(2),
            unicast: Vec::new(),
        };
        std::thread::spawn(move || {
            let live = shared.clone();
            let result = run_probe(&cfg, |s| {
                lock(&live).lines.push(format!(
                    "probe: first {} frame from {}{}",
                    s.kind.label(),
                    s.from,
                    if s.verified { "" } else { " (other code)" }
                ));
            });
            let mut s = lock(&shared);
            match result {
                Ok(report) => {
                    let ok = report.key_exchange_done();
                    s.lines = report.lines();
                    s.lines.push(format!(
                        "probe: result: {}",
                        if ok { "ok" } else { "failed" }
                    ));
                }
                Err(e) => {
                    s.lines = vec![
                        format!("probe: error: {e}"),
                        "probe: result: failed".to_string(),
                    ]
                }
            }
            s.running = false;
        });
        Ok(code.as_str().to_string())
    }
}
