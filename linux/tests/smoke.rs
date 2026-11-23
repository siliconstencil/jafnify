#[test]
fn crate_loads_and_exposes_app_dir() {
    assert_eq!(jafnify_linux::APP_DIR, "jafnify");
    let _ = jafnify_core::state::Role::Joiner;
}
