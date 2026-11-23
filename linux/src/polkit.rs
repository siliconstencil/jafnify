//! The per-user polkit rule Jafnify installs at adoption.
//!
//! The actions' `.policy` keeps its defaults; this rule only makes the adopting
//! user's local session prompt-free for `org.jafnify.tunnel` (while active) and
//! for `org.jafnify.route` (also when inactive, so the route fallback can run at
//! logout). It never broadens an action or grants any other program root.

use std::ffi::CStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const RULES_PATH: &str = crate::flavor::RULES_PATH;
pub const RULE_FILE: &str = crate::flavor::RULE_FILE;
pub const ACTION: &str = crate::flavor::ACTION_TUNNEL;
pub const ACTION_ROUTE: &str = crate::flavor::ACTION_ROUTE;

/// A polkit rule string embeds the name in JavaScript. Keep it to the
/// conservative Unix login-name alphabet so quotes, newlines and escapes
/// cannot change the rule.
pub fn validate_user(user: &str) -> Result<(), String> {
    if user.is_empty() || user.len() > 64 {
        return Err("bad user name".to_string());
    }
    let ok = user
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    if ok {
        Ok(())
    } else {
        Err("bad user name".to_string())
    }
}

/// The exact rule text. The action ids are fixed; only the adopting user
/// varies. The route grant has no `subject.active` check: the logout fallback
/// has to run after the session became inactive.
pub fn rule_text(user: &str) -> Result<String, String> {
    validate_user(user)?;
    Ok(format!(
        "// Installed by Jafnify at adoption. Removed by forget or uninstall.\n\
         polkit.addRule(function(action, subject) {{\n\
         \x20   if (action.id == \"{ACTION}\" &&\n\
         \x20       subject.user == \"{user}\" &&\n\
         \x20       subject.local &&\n\
         \x20       subject.active) {{\n\
         \x20       return polkit.Result.YES;\n\
         \x20   }}\n\
         }});\n\
         polkit.addRule(function(action, subject) {{\n\
         \x20   if (action.id == \"{ACTION_ROUTE}\" &&\n\
         \x20       subject.user == \"{user}\" &&\n\
         \x20       subject.local) {{\n\
         \x20       return polkit.Result.YES;\n\
         \x20   }}\n\
         }});\n"
    ))
}

pub fn rules_present(path: &Path) -> bool {
    path.is_file()
}

/// Write the rule atomically with mode 0644. If the same content is already
/// there, leave the file alone so polkit does not reload for nothing.
pub fn install_at(path: &Path, user: &str) -> std::io::Result<()> {
    let text =
        rule_text(user).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    if fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    let dir = path
        .parent()
        .unwrap_or_else(|| Path::new("/etc/polkit-1/rules.d"));
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{RULE_FILE}.jafnify"));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o644)
        .open(&tmp)?;
    file.set_permissions(fs::Permissions::from_mode(0o644))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    Ok(())
}

pub fn remove_at(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// The login name for a uid, if the account exists and the name is safe to
/// embed in the JavaScript rule. `getpwuid_r` writes into this call's buffer,
/// so two threads cannot race on libc's static storage.
fn user_name(uid: u32) -> Option<String> {
    let mut buffer = vec![0u8; 4096];
    // SAFETY: `getpwuid_r` writes the passwd record into `pwd` and its strings
    // into `buffer`, which stays alive and untouched until the copy below. All
    // pointers are valid for the size given.
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buffer.as_mut_ptr().cast::<libc::c_char>(),
            buffer.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        // Not found, or the record did not fit: no name to embed.
        return None;
    }
    // SAFETY: `result` names `pwd`, and `pw_name` points into `buffer`, which
    // is still alive and was written by the successful call above.
    let name = unsafe {
        if pwd.pw_name.is_null() {
            return None;
        }
        CStr::from_ptr(pwd.pw_name).to_str().ok()?.to_string()
    };
    validate_user(&name).ok()?;
    Some(name)
}

/// The user running this app. The helper installs the rule for the caller, and
/// `invoking_user` must never be used for the unprivileged app's own tests.
pub fn current_user() -> Option<String> {
    let uid = unsafe { libc::getuid() };
    user_name(uid)
}

/// The user whose per-user rule `install-helper` rewrites: the login user, or
/// under sudo/pkexec the caller (`SUDO_USER` / `PKEXEC_UID`). Never root: a
/// root-only run rewrites no rule.
pub fn rule_owner_for_install() -> Option<String> {
    let uid = unsafe { libc::getuid() };
    let name = if uid == 0 {
        match std::env::var("SUDO_USER") {
            Ok(u) if validate_user(&u).is_ok() => Some(u),
            _ => invoking_user(),
        }
    } else {
        user_name(uid)
    };
    name.filter(|u| u != "root" && validate_user(u).is_ok())
}

/// The pkexec caller's login name, if pkexec set `PKEXEC_UID` and the account
/// exists. A direct root run has no user and installs no rule.
pub fn invoking_user() -> Option<String> {
    let uid: u32 = std::env::var("PKEXEC_UID").ok()?.parse().ok()?;
    user_name(uid)
}

pub fn default_path() -> PathBuf {
    PathBuf::from(RULES_PATH)
}
