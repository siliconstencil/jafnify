use std::os::unix::fs::PermissionsExt;

use jafnify_linux::polkit;

/// The one action block for `id` in the shipped policy file.
fn action_block<'a>(policy: &'a str, id: &str) -> &'a str {
    let marker = format!("<action id=\"{id}\">");
    let start = policy.find(&marker).expect("action is declared");
    let rest = &policy[start..];
    let end = rest.find("</action>").expect("action is closed") + "</action>".len();
    &rest[..end]
}

#[test]
fn the_route_action_is_password_free_and_bound_to_its_own_binary() {
    let policy = include_str!("../../assets/polkit/org.jafnify.policy");
    let route = action_block(policy, "org.jafnify.route");
    assert!(
        route.contains("<allow_active>yes</allow_active>"),
        "{route}"
    );
    assert!(
        route.contains("/usr/libexec/jafnify/jafnify-route"),
        "{route}"
    );
    assert!(route.contains("<allow_any>no</allow_any>"), "{route}");
    assert!(
        route.contains("<allow_inactive>no</allow_inactive>"),
        "{route}"
    );
    // The tunnel action keeps its password and its own helper.
    let tunnel = action_block(policy, "org.jafnify.tunnel");
    assert!(
        tunnel.contains("<allow_active>auth_admin_keep</allow_active>"),
        "{tunnel}"
    );
    assert!(
        tunnel.contains("/usr/libexec/jafnify/jafnify-helper"),
        "{tunnel}"
    );
}

/// The rule blocks in order: the tunnel grant first, the route grant second.
fn rule_blocks(text: &str) -> Vec<&str> {
    text.split("polkit.addRule").skip(1).collect()
}

#[test]
fn the_rule_allows_only_the_adopting_active_local_user() {
    let text = polkit::rule_text("alice").unwrap();
    let blocks = rule_blocks(&text);
    assert_eq!(blocks.len(), 2, "{text}");
    let (tunnel, route) = (blocks[0], blocks[1]);
    assert!(
        tunnel.contains("action.id == \"org.jafnify.tunnel\""),
        "{tunnel}"
    );
    assert!(tunnel.contains("subject.user == \"alice\""), "{tunnel}");
    assert!(tunnel.contains("subject.local"), "{tunnel}");
    assert!(tunnel.contains("subject.active"), "{tunnel}");
    assert!(tunnel.contains("polkit.Result.YES"), "{tunnel}");
    assert!(
        route.contains("action.id == \"org.jafnify.route\""),
        "{route}"
    );
    assert!(route.contains("subject.user == \"alice\""), "{route}");
    assert!(route.contains("subject.local"), "{route}");
    assert!(!text.contains("allow_any"), "{text}");
    assert!(!text.contains("polkit.Result.NO"), "{text}");
}

#[test]
fn the_route_grant_follows_the_same_user_even_when_inactive() {
    // The route action is allow_inactive=no, so at logout the
    // shutdown fallback could not run for the now-inactive session. The
    // per-user rule closes that gap for the adopting user only, with no
    // `subject.active` check. The shipped policy defaults stay as they are.
    let text = polkit::rule_text("alice").unwrap();
    let route = rule_blocks(&text)[1];
    assert!(route.contains("org.jafnify.route"), "{route}");
    assert!(route.contains("subject.local"), "{route}");
    assert!(!route.contains("subject.active"), "{route}");
    let policy = include_str!("../../assets/polkit/org.jafnify.policy");
    let action = action_block(policy, "org.jafnify.route");
    assert!(
        action.contains("<allow_inactive>no</allow_inactive>"),
        "{action}"
    );
    assert!(action.contains("<allow_any>no</allow_any>"), "{action}");
}

#[test]
fn install_replaces_an_old_rule_that_lacks_the_route_grant() {
    // An upgrade keeps the file name, so the install step must rewrite the
    // old tunnel-only content with the two-block rule.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.d/49-jafnify.rules");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let old = "// Installed by Jafnify at adoption. Removed by forget or uninstall.\n\
polkit.addRule(function(action, subject) {\n\
    if (action.id == \"org.jafnify.tunnel\" &&\n\
        subject.user == \"alice\" &&\n\
        subject.local &&\n\
        subject.active) {\n\
        return polkit.Result.YES;\n\
    }\n\
});\n";
    std::fs::write(&path, old).unwrap();
    polkit::install_at(&path, "alice").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, polkit::rule_text("alice").unwrap());
    assert!(text.contains("org.jafnify.route"), "{text}");
    assert!(text.contains("org.jafnify.tunnel"), "{text}");
}

#[test]
fn a_user_name_that_could_break_the_javascript_is_rejected() {
    for user in ["", "alice\"", "alice\n", "alice\\", "alice bob", "alice;x"] {
        assert!(polkit::rule_text(user).is_err(), "accepted {user:?}");
    }
    assert!(polkit::validate_user("alice").is_ok());
    assert!(polkit::validate_user("alice.smith-1_2").is_ok());
}

#[test]
fn install_writes_0644_and_replaces_an_older_user() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.d/49-jafnify.rules");

    polkit::install_at(&path, "alice").unwrap();
    let meta = std::fs::metadata(&path).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o644);
    assert!(polkit::rules_present(&path));
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("\"alice\""));

    polkit::install_at(&path, "bob").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"bob\""), "{text}");
    assert!(!text.contains("\"alice\""), "{text}");
}

#[test]
fn remove_deletes_the_file_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.d/49-jafnify.rules");
    polkit::install_at(&path, "alice").unwrap();
    polkit::remove_at(&path).unwrap();
    assert!(!path.exists());
    assert!(!polkit::rules_present(&path));
    polkit::remove_at(&path).unwrap();
}

#[test]
fn an_existing_rule_is_compared_before_rewriting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.d/49-jafnify.rules");
    polkit::install_at(&path, "alice").unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    polkit::install_at(&path, "alice").unwrap();
    let after = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(before, after);
}
