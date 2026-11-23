//! The app log lives under the flavor's own state directory, so a side-by-side
//! test build never writes into the product's log directory.

#[test]
fn app_log_dir_follows_the_flavor() {
    let path = jafnify_linux::applog::default_path();
    let dir = path.parent().and_then(|p| p.file_name()).unwrap();
    assert_eq!(dir, jafnify_linux::flavor::NAME);
    assert_eq!(path.file_name().unwrap(), "jafnify.log");
}
