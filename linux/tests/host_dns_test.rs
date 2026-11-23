//! The host DNS preflight is a read-only parser. A stale resolved.conf.d
//! drop-in can take every lookup away from the hotspot while the tunnel is
//! down (an incident); Jafnify never writes one, but it should say
//! that the host has one. These tests use fixture files; they never touch the
//! real resolver.

use std::path::{Path, PathBuf};

use jafnify_linux::host_dns::{find, scan_file, DnsReason};

fn write(root: &Path, rel: &str, body: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn the_incident_drop_in_is_reported_with_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "etc/systemd/resolved.conf.d/dns64.conf",
        "[Resolve]\nDNS=2001:db8::1\nDomains=~.\n",
    );
    let found = find(dir.path()).expect("the drop-in must be reported");
    assert_eq!(found.file, path);
    assert_eq!(found.reason, DnsReason::Domains);
    assert_eq!(
        found.line(),
        format!(
            "System DNS override in {} may break the internet when the tunnel is down",
            path.display()
        )
    );
}

#[test]
fn a_route_all_domains_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "etc/systemd/resolved.conf",
        "[Resolve]\nDomains=example.test ~.\n",
    );
    let found = find(dir.path()).expect("the route-all domain must be reported");
    assert_eq!(found.reason, DnsReason::Domains);
}

#[test]
fn an_empty_dns_line_is_not_an_override() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "etc/systemd/resolved.conf",
        "[Resolve]\nDNS=\nDomains=lan\n",
    );
    assert!(find(dir.path()).is_none());
}

#[test]
fn a_dns_key_outside_the_resolve_section_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "etc/systemd/resolved.conf",
        "[Other]\nDNS=1.1.1.1\n",
    );
    assert_eq!(scan_file(&path), None);
}

#[test]
fn a_backup_file_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "etc/systemd/resolved.conf.d/dns64.conf.bak-20261106",
        "[Resolve]\nDNS=2001:db8::1\nDomains=~.\n",
    );
    assert!(find(dir.path()).is_none());
}

#[test]
fn the_etc_drop_in_wins_over_a_clean_usr_lib_base() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "usr/lib/systemd/resolved.conf", "[Resolve]\n");
    let offending = write(
        dir.path(),
        "etc/systemd/resolved.conf.d/override.conf",
        "[Resolve]\nDomains = ~.\n",
    );
    assert_eq!(find(dir.path()).unwrap().file, offending);
}

#[test]
fn a_global_dns_server_alone_is_a_normal_choice_and_not_flagged() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "etc/systemd/resolved.conf.d/privacy.conf",
        "[Resolve]\nDNS=1.1.1.1 9.9.9.9\nFallbackDNS=8.8.8.8\n",
    );
    assert!(find(dir.path()).is_none());
}
