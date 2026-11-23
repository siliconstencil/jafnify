//! systemd user-unit autostart. Default OFF; toggled from the GUI.
//! The unit file lives under ~/.config/systemd/user/ and needs no root.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::exec;

#[derive(Debug, thiserror::Error)]
pub enum AutostartError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("systemctl failed: {0}")]
    Systemctl(String),
    #[error("Autostart is blocked on this system (the jafnify user service is masked). To allow it, run: systemctl --user unmask jafnify.service")]
    Masked,
}

const UNIT_TEMPLATE: &str = "[Unit]\nDescription=Jafnify P2P tunnel\nAfter=graphical-session.target\nPartOf=graphical-session.target\n\n[Service]\nType=simple\nExecStart={exec}\nRestart=on-failure\nExecStopPost={stop} fallback\n\n[Install]\nWantedBy=graphical-session.target\n";

pub fn write_unit_file(service_path: &Path, exec_line: &str) -> Result<(), AutostartError> {
    if let Some(parent) = service_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bin = exec_line.split_whitespace().next().unwrap_or(exec_line);
    let content = UNIT_TEMPLATE
        .replace("{exec}", exec_line)
        .replace("{stop}", bin);
    let mut f = fs::File::create(service_path)?;
    f.write_all(content.as_bytes())?;
    Ok(())
}

pub fn is_masked(service_path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(service_path) else {
        return false;
    };
    if !meta.file_type().is_symlink() {
        return false;
    }
    matches!(fs::read_link(service_path), Ok(target) if target == Path::new("/dev/null"))
}

fn says_masked(systemctl_bin: &Path) -> Result<bool, AutostartError> {
    let mut cmd = Command::new(systemctl_bin);
    cmd.args(["--user", "is-enabled", crate::flavor::UNIT]);
    let out = exec::output(cmd)?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| line.trim() == "masked"))
}

pub fn enable(
    service_path: &Path,
    exec_line: &str,
    systemctl_bin: &Path,
) -> Result<(), AutostartError> {
    if is_masked(service_path) || says_masked(systemctl_bin)? {
        return Err(AutostartError::Masked);
    }
    write_unit_file(service_path, exec_line)?;
    let mut cmd = Command::new(systemctl_bin);
    cmd.args(["--user", "enable", crate::flavor::UNIT]);
    let out = exec::output(cmd)?;
    if !out.status.success() {
        return Err(AutostartError::Systemctl(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(())
}

pub fn disable(_service_path: &Path, systemctl_bin: &Path) -> Result<(), AutostartError> {
    let mut cmd = Command::new(systemctl_bin);
    cmd.args(["--user", "disable", crate::flavor::UNIT]);
    let out = exec::output(cmd)?;
    if !out.status.success() {
        return Err(AutostartError::Systemctl(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ));
    }
    Ok(())
}
