//! Plain-text clipboard writes without a new dependency.
//!
//! The daemon owns the tray, so a tray menu item cannot use egui's clipboard
//! context. It asks one of the standard command-line owners instead:
//! `wl-copy` on Wayland, `xclip` or `xsel` on X11. The first one that accepts
//! the text wins; a missing tool is not an error until every candidate failed.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::process::{Command, Stdio};

/// Write `text` to the session clipboard with the first available tool.
pub fn copy(text: &str) -> Result<(), String> {
    copy_with(text, std::env::var_os("PATH").as_deref())
}

/// The testable form: run the candidates with an explicit `PATH`.
pub fn copy_with(text: &str, path: Option<&OsStr>) -> Result<(), String> {
    let candidates: [(&str, &[&str]); 3] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    let mut last = None;
    for (program, args) in candidates {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(path) = path {
            command.env("PATH", path);
        }
        match command.spawn() {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    if let Err(e) = stdin.write_all(text.as_bytes()) {
                        let _ = child.wait();
                        last = Some(format!("{program}: {e}"));
                        continue;
                    }
                }
                match child.wait() {
                    Ok(status) if status.success() => return Ok(()),
                    Ok(status) => last = Some(format!("{program} exited with {status}")),
                    Err(e) => last = Some(format!("{program}: {e}")),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                last = Some(format!("{program} is not installed"));
            }
            Err(e) => last = Some(format!("{program}: {e}")),
        }
    }
    Err(last.unwrap_or_else(|| "no clipboard tool".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn fake_script(dir: &Path, name: &str, body: &Path) {
        let path = dir.join(name);
        let text = format!(
            "#!/bin/sh\n/bin/cat > '{}'\n",
            body.to_string_lossy().replace('\'', "'\\''")
        );
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn the_first_available_tool_receives_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let body = dir.path().join("clipboard.txt");
        fake_script(dir.path(), "xclip", &body);

        copy_with("Jafnify diagnostics\n", Some(dir.path().as_os_str())).unwrap();

        assert_eq!(fs::read_to_string(&body).unwrap(), "Jafnify diagnostics\n");
    }

    #[test]
    fn a_missing_tool_is_reported_when_every_candidate_fails() {
        let dir = tempfile::tempdir().unwrap();
        let error = copy_with("text", Some(dir.path().as_os_str())).unwrap_err();
        assert!(error.contains("is not installed"), "{error}");
    }
}
