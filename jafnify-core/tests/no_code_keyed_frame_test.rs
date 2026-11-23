//! No build path may put a frame on the wire that the pairing code alone
//! verifies (P0-1).
//!
//! The old 0x04 pairing frame is exactly that: its MAC is
//! HMAC-SHA256(code, public_key || port), so a captured frame can be
//! brute-forced offline against the 9,000 possible codes. The same is true of
//! the old 0x01 presence frame (HMAC(code, nonce)). The SPAKE2 pairing (0x05
//! and 0x06) replaced both.
//!
//! This test scans the production sources of the whole tree and fails when a
//! send path for such a frame appears. Receiving and recognising a legacy
//! frame (to report "update the other device") is allowed: that is what
//! `handshake.rs` and `pairing.rs` are for.

use std::fs;
use std::path::{Path, PathBuf};

/// The repository root: the core crate's manifest directory's parent.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the core crate lives in the repository")
        .to_path_buf()
}

/// The production part of a file: everything before its `#[cfg(test)]` module.
fn production_part(body: &str) -> &str {
    body.split("#[cfg(test)]").next().unwrap_or(body)
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

fn java_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            java_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("java") {
            out.push(path);
        }
    }
}

/// The production sources of the whole tree: the core, the Linux app, the
/// Android JNI and the Android app.
fn production_sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("jafnify-core/src"), &mut files);
    rust_files(&root.join("linux/src"), &mut files);
    rust_files(&root.join("app/wgserv/src"), &mut files);
    java_files(&root.join("app/app/src/main/java"), &mut files);
    let mut out: Vec<(String, String)> = files
        .into_iter()
        .map(|p| {
            let name = p
                .strip_prefix(&root)
                .unwrap_or(&p)
                .to_string_lossy()
                .to_string();
            let body = fs::read_to_string(&p).expect("read source");
            (name, production_part(&body).to_string())
        })
        .collect();
    out.sort();
    out
}

/// The code as an HMAC key. Only the legacy receive paths may do this.
const CODE_AS_MAC_KEY: &str = "new_from_slice(code.as_str().as_bytes())";

/// The files that may key a MAC with the code: both are receive-only.
const LEGACY_RECEIVE_PATHS: [&str; 2] = [
    "jafnify-core/src/handshake.rs",
    "jafnify-core/src/pairing.rs",
];

#[test]
fn no_production_source_keys_a_mac_with_the_pairing_code_outside_the_legacy_receive_paths() {
    let sources = production_sources();
    assert!(
        sources.len() > 10,
        "the scan found almost nothing: {:?}",
        sources.len()
    );
    for (name, body) in &sources {
        if body.contains(CODE_AS_MAC_KEY) {
            assert!(
                LEGACY_RECEIVE_PATHS.contains(&name.as_str()),
                "{name} keys a MAC with the pairing code: a frame of that kind is \
                 brute-forceable offline (P0-1)"
            );
        }
    }
}

#[test]
fn no_production_source_builds_a_legacy_pairing_frame() {
    for (name, body) in production_sources() {
        if body.contains("HandshakeAnnouncement") {
            // The type itself may only live in the receive path, where a
            // legacy frame is parsed to be recognised (its builders are gone,
            // see the handshake module test).
            assert!(
                name == "jafnify-core/src/handshake.rs",
                "{name} builds a legacy 0x04 frame: its MAC is keyed with the pairing code (P0-1)"
            );
        }
    }
}

#[test]
fn the_legacy_handshake_module_has_no_builder_and_no_sender() {
    let sources = production_sources();
    let handshake = sources
        .iter()
        .find(|(n, _)| n == "jafnify-core/src/handshake.rs")
        .map(|(_, b)| b.as_str())
        .expect("the handshake module");
    assert!(
        !handshake.contains("fn new("),
        "handshake.rs builds a legacy frame again"
    );
    assert!(
        !handshake.contains("fn new_with_port("),
        "handshake.rs builds a legacy frame again"
    );
    assert!(
        !handshake.contains("fn announce("),
        "handshake.rs can send a legacy frame again"
    );
    // The MAC helper has exactly one caller: `verify`.
    assert_eq!(
        handshake.matches("mac_over(").count(),
        2,
        "the legacy MAC helper gained a caller"
    );
}

#[test]
fn the_legacy_presence_tag_is_only_verified_and_never_built_from_the_code() {
    let sources = production_sources();
    let pairing = sources
        .iter()
        .find(|(n, _)| n == "jafnify-core/src/pairing.rs")
        .map(|(_, b)| b.as_str())
        .expect("the pairing module");
    // The presence tag is built with the public discovery key.
    assert!(
        pairing.contains("PUBLIC_DISCOVERY_KEY"),
        "the presence tag is not built with the public key any more"
    );
    // The code-keyed tag builder is gone; only the verifier is left.
    assert!(
        !pairing.contains("fn derive_legacy"),
        "the code-keyed presence builder is back"
    );
    assert!(
        pairing.contains("fn verify_legacy"),
        "the legacy presence verifier is gone: an old peer would not be recognised"
    );
}

#[test]
fn only_the_receive_path_verifies_the_legacy_presence_tag() {
    for (name, body) in production_sources() {
        if body.contains("verify_legacy(") {
            assert!(
                name == "jafnify-core/src/pairing.rs" || name == "jafnify-core/src/orchestrator.rs",
                "{name} verifies the legacy presence tag: it must be a receive path"
            );
        }
    }
}
