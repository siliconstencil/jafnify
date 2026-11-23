//! Every lab script uses the one joiner sandbox prelude.

use std::path::Path;

fn lab_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/lab")
}

fn scripts() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(lab_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("e2e-") || n == "repro-solo.sh")
        .filter(|n| n.ends_with(".sh"))
        .collect();
    names.sort();
    names
}

#[test]
fn every_lab_script_sources_the_shared_prelude() {
    for name in scripts() {
        let text = std::fs::read_to_string(lab_dir().join(&name)).unwrap();
        assert!(
            text.contains(". \"$(dirname \"$0\")/lib.sh\""),
            "{name} must source tools/lab/lib.sh"
        );
        assert!(
            !text.contains("unshare -m") && !text.contains("mount -t tmpfs"),
            "{name} must use joiner_sandbox from lib.sh, not its own mount namespace"
        );
    }
}

#[test]
fn no_lab_script_runs_the_joiner_outside_the_mount_sandbox() {
    for name in scripts() {
        let text = std::fs::read_to_string(lab_dir().join(&name)).unwrap();
        let mut logical = String::new();
        let mut start = 1;
        for (i, line) in text.lines().enumerate() {
            if logical.is_empty() {
                start = i + 1;
            }
            let continued = line.trim_end().ends_with('\\');
            let part = line.trim_end().strip_suffix('\\').unwrap_or(line);
            logical.push_str(part);
            if continued {
                logical.push(' ');
                continue;
            }
            let trimmed = logical.trim_start();
            if !trimmed.starts_with('#')
                && trimmed.contains("ip netns exec")
                && !trimmed.contains("joiner_sandbox")
                && (trimmed.contains("$JOIN")
                    || trimmed.contains("jafnify")
                    || trimmed.contains("jafnify-helper"))
            {
                panic!("{name}:{start} runs the joiner outside joiner_sandbox: {logical}");
            }
            logical.clear();
        }
    }
}

#[test]
fn the_prelude_has_the_required_sandbox_and_host_dns_check() {
    let text = std::fs::read_to_string(lab_dir().join("lib.sh")).unwrap();
    assert!(text.contains("mount -t tmpfs tmpfs /etc/wireguard"));
    assert!(text.contains("mount -t tmpfs tmpfs /etc/polkit-1"));
    assert!(text.contains("/run/systemd/resolve"));
    assert!(text.contains("resolvconf"));
    assert!(text.contains("resolvectl status"));
    assert!(text.contains("host-resolved-before"));
    assert!(text.contains("host resolvectl status unchanged"));
    assert!(text.contains("host_sensitive_digest"));
    assert!(text.contains("host-sensitive-before"));
    assert!(text.contains("host-sensitive-after"));
    assert!(text.contains("host sensitive files unchanged"));
    assert!(text.contains("io.systemd.Resolve"));
    assert!(text.contains("bin/resolvectl"));
    assert!(text.contains("no joiner log contains resolvectl"));
}

#[test]
fn every_lab_script_is_valid_shell() {
    for name in scripts() {
        let out = std::process::Command::new("bash")
            .args(["-n", lab_dir().join(&name).to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn the_link_lab_runs_a_delayed_and_a_lost_link_test() {
    let text = std::fs::read_to_string(lab_dir().join("e2e-link.sh")).unwrap();
    assert!(text.contains("link-test"), "no `jafnify link-test` step");
    assert!(text.contains("average"), "no average assertion");
    assert!(
        text.contains("128") && text.contains("192"),
        "the 160 ms +/- 20 percent band is missing"
    );
    assert!(text.contains("No reply"), "no 100 percent loss case");
}

#[test]
fn the_link_lab_polls_for_the_joiner_connected_state() {
    // The one-shot grep raced the joiner log once (the tunnel was
    // already up). The check must poll, with the lab's tunnel timeout.
    let text = std::fs::read_to_string(lab_dir().join("e2e-link.sh")).unwrap();
    assert!(
        text.contains("wait_connected() {"),
        "e2e-link.sh needs a wait_connected poll helper"
    );
    let body = text
        .split("wait_connected() {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .unwrap_or("");
    assert!(
        body.contains("while ["),
        "wait_connected must poll, not grep once: {body}"
    );
    assert!(
        body.contains("sleep"),
        "wait_connected must wait between polls: {body}"
    );
    assert!(
        text.contains("wait_connected 90"),
        "the Connected poll must use the 90 s tunnel timeout"
    );
    assert!(
        !text.contains("if grep -q Connected \"$OUT/joiner.log\"; then"),
        "the one-shot Connected grep is the race that was fixed"
    );
}

#[test]
fn the_known_lab_polls_for_the_joiner_connected_state() {
    // The same one-shot grep race as e2e-link.sh. The adopt step
    // must poll a helper, so a line written a moment later is not missed.
    let text = std::fs::read_to_string(lab_dir().join("e2e-known.sh")).unwrap();
    assert!(
        text.contains("wait_connected() {"),
        "e2e-known.sh needs a wait_connected poll helper"
    );
    let body = text
        .split("wait_connected() {")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .unwrap_or("");
    assert!(
        body.contains("while ["),
        "wait_connected must poll, not grep once: {body}"
    );
    assert!(
        body.contains("sleep"),
        "wait_connected must wait between polls: {body}"
    );
    assert!(
        text.contains("wait_connected 30 \"$OUT/joiner-adopt-second.log\""),
        "the Connected poll must use the adopt log"
    );
    assert!(
        !text.contains("grep -q Connected \"$OUT/joiner-adopt-second.log\" && echo"),
        "the one-shot Connected grep is the race that was fixed"
    );
}

#[test]
fn the_resilience_lab_encodes_peer_loss_and_suppress_repair() {
    let text = std::fs::read_to_string(lab_dir().join("e2e-resilience.sh")).unwrap();
    assert!(
        text.contains("a restarted sharer leads to exactly one tunnel rebuild"),
        "{text}"
    );
    assert!(text.contains("adoption rule is present"), "{text}");
    assert!(
        text.contains("adopted only after inspect and before Connected"),
        "{text}"
    );
    assert!(
        text.contains("missing config teardown removes jafn0"),
        "{text}"
    );
    assert!(text.contains("state Connecting -> Connected"), "{text}");
    assert!(
        text.contains("pair not persisted before deleting config"),
        "{text}"
    );
    assert!(
        text.contains("next connect recreates config and routing rule"),
        "{text}"
    );
    assert!(
        text.contains("suppress rule removed externally is restored"),
        "{text}"
    );
    // G2: the password-free, mark-scoped fallback and its lab cases.
    for case in [
        "jafn0's own fwmark rule is gone",
        "jafn0's fwmark rule is back",
        "the IPv6 blackhole is back",
        "the return used the password-free route binary",
        "the other tunnel's fwmark rule survived the fallback",
        "the other tunnel's traffic still goes through it after the fallback",
        "tray mode SIGTERM leaves the internet reachable",
        "tray stop leaves no Jafnify rule behind",
        "no Jafnify rule is left behind",
    ] {
        assert!(text.contains(case), "missing case: {case}");
    }
    assert!(text.contains("jafnify-route"), "{text}");
}

#[test]
fn the_away_lab_proves_a_long_absence_is_not_a_drop() {
    let text = std::fs::read_to_string(lab_dir().join("e2e-away-return.sh")).unwrap();
    for needle in [
        // The drop history switch is on, in the private flags of the lab.
        "drop_history = true",
        // The absence is longer than the edge of the core.
        "AWAY_SECONDS",
        // The record of the absence.
        "away at=",
        "the absence is recorded as an away event",
        // The laptop comes back on its own.
        "connected again with no user action",
        // The count the user sees.
        "Drops in the last 24 h: 0",
        // The run fails when a case fails, and it checks the host at the end.
        "exit $FAILS",
        "lab_finish",
        // The length of the absence is measured, not assumed.
        "MEASURED",
    ] {
        assert!(
            text.contains(needle),
            "{needle} is missing from e2e-away-return.sh"
        );
    }
}

#[test]
fn the_multinic_lab_requires_two_networks_and_a_followed_hotspot() {
    // F5: the search must follow the network that leads to the phone, not the
    // second network that happens to have the default route.
    let text = std::fs::read_to_string(lab_dir().join("e2e-multinic.sh")).unwrap();
    for needle in [
        "two networks",
        "the search follows the hotspot",
        "without a restart",
        "AWAY_SECONDS",
        "RECONNECT_SECONDS",
        "M5 no restart",
        "exit $FAILS",
        "lab_finish",
    ] {
        assert!(
            text.contains(needle),
            "{needle} is missing from e2e-multinic.sh"
        );
    }
}

#[test]
fn the_prelude_copies_the_route_binary_into_the_sandbox() {
    let text = std::fs::read_to_string(lab_dir().join("lib.sh")).unwrap();
    assert!(text.contains("LAB_ROUTE"), "{text}");
    assert!(
        text.contains("/usr/libexec/jafnify/jafnify-route"),
        "{text}"
    );
}

fn bad_single_quote_in_joiner_bash_c(text: &str) -> Option<usize> {
    let mut line_start = 0;
    for line in text.lines() {
        if !line.trim_start().starts_with('#')
            && line.contains("joiner_sandbox")
            && line.contains("bash -c '")
        {
            let open = line_start + line.find("bash -c '").unwrap() + "bash -c '".len();
            if let Some(offset) = text[open..].find('\'') {
                let at = open + offset;
                let quote_line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
                let mid_line = !text[quote_line_start..at].trim().is_empty();
                let after = &text[at + 1..];
                let next = after.trim_start_matches([' ', '\t']).chars().next();
                let valid_mid_line = matches!(next, Some('"') | Some('_') | Some('&'));
                if mid_line && !valid_mid_line {
                    return Some(at);
                }
                let valid = matches!(
                    next,
                    None | Some('\n')
                        | Some('\r')
                        | Some('_')
                        | Some('"')
                        | Some('&')
                        | Some(')')
                        | Some(';')
                        | Some('|')
                        | Some('>')
                        | Some('<')
                );
                if !valid {
                    return Some(at);
                }
            }
        }
        line_start += line.len() + 1;
    }
    None
}

#[test]
fn joiner_sandbox_bash_c_blocks_do_not_contain_single_quotes() {
    let bad = "joiner_sandbox jl-jo bash -c '\n  grep -q 'state Connecting -> Connected' \"$log\"\n' _ \"$1\"\n";
    let good = "joiner_sandbox jl-jo bash -c '\n  grep -q \"state Connecting -> Connected\" \"$log\"\n' _ \"$1\"\n";
    assert!(bad_single_quote_in_joiner_bash_c(bad).is_some());
    assert!(bad_single_quote_in_joiner_bash_c(good).is_none());

    for name in scripts() {
        let text = std::fs::read_to_string(lab_dir().join(&name)).unwrap();
        if let Some(at) = bad_single_quote_in_joiner_bash_c(&text) {
            let line = text[..at].lines().count();
            panic!("{name}: single quote inside joiner_sandbox bash -c at line {line}");
        }
    }
}

#[test]
fn the_phone_stats_lab_covers_the_channel_and_the_lan_stranger() {
    let text = std::fs::read_to_string(lab_dir().join("e2e-phonestats.sh")).unwrap();
    for needle in [
        "--stats-fake-line",
        "phone-stats",
        "10.233.0.1",
        "192.168.77.1",
        "JFS1 1 summary",
        "paired joiner reads the fake summary",
        "stranger replies",
        "rate replies",
        "no provider",
    ] {
        assert!(
            text.contains(needle),
            "{needle} is missing from e2e-phonestats.sh"
        );
    }
}

#[test]
fn the_sandbox_body_is_one_valid_single_quoted_script() {
    // A single quote inside joiner_sandbox's `bash -c '...'`
    // body ended the string early; every lab joiner then failed to start and
    // the scripts only reported their host checks. `bash -n` on lib.sh alone
    // cannot see this, so check the body itself.
    let text = std::fs::read_to_string(lab_dir().join("lib.sh")).unwrap();
    let seg = &text[text.find("joiner_sandbox() {").expect("joiner_sandbox")..];
    let start = seg.find("bash -c '").expect("bash -c body") + "bash -c '".len();
    let end = seg.find("' sh \"$@\"").expect("end of body");
    let body = &seg[start..end];
    assert!(
        !body.contains('\''),
        "a single quote ends the sandbox body early"
    );
    let mut child = std::process::Command::new("bash")
        .arg("-n")
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
