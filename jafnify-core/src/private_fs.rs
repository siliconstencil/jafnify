//! Private files and directories, one shared helper for every store.
//!
//! Unix: a created directory is 0700 and a new file is 0600. Windows: both are
//! created with a protected, owner-only DACL (full access for the current
//! user's SID, no inheritance), so a later store (connection log, drops,
//! usage) gets the same protection by calling this module. An existing
//! directory is never tightened, on either platform.

use std::fs;
use std::path::Path;

/// Creates a directory only this user can enter: 0700 on Unix; on Windows a
/// protected DACL granting full access to this user only.
#[cfg(unix)]
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

/// Creates a new file only this user can read: 0600 on Unix; on Windows a
/// protected DACL granting full access to this user only.
#[cfg(unix)]
pub fn open_private_file(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::io::FromRawHandle;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, GENERIC_WRITE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
};
#[cfg(windows)]
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
#[cfg(windows)]
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, CreateFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Creates each missing ancestor with the same owner-only DACL, then the
/// directory itself. An existing directory is left alone.
#[cfg(windows)]
pub fn create_private_dir(path: &Path) -> std::io::Result<()> {
    if path.as_os_str().is_empty() || path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            create_private_dir(parent)?;
        }
    }
    let acl = OwnerOnlyDacl::new()?;
    let attributes = acl.attributes();
    let wide = wide_path(path);
    // SAFETY: `wide` is NUL-terminated and outlives the call; `attributes`
    // points at a live security descriptor for the same call.
    let created = unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) };
    if created == 0 {
        let error = std::io::Error::last_os_error();
        // Another thread may have created it between the check and the call.
        if error.raw_os_error() == Some(183) && path.is_dir() {
            return Ok(());
        }
        return Err(error);
    }
    Ok(())
}

/// Creates a new file with a protected DACL granting full access to this user
/// only. Fails if the path already exists (`CREATE_NEW`).
#[cfg(windows)]
pub fn open_private_file(path: &Path) -> std::io::Result<fs::File> {
    let acl = OwnerOnlyDacl::new()?;
    let attributes = acl.attributes();
    let wide = wide_path(path);
    // SAFETY: `wide` is NUL-terminated and outlives the call; `attributes`
    // points at a live security descriptor for the same call. `CreateFileW`
    // returns an owned handle that `File` takes over.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle == 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the handle came from CreateFileW just above and is not used
    // anywhere else, so `File` owns it.
    Ok(unsafe { fs::File::from_raw_handle(handle as *mut std::ffi::c_void) })
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// A security descriptor allocated by `ConvertStringSecurityDescriptor...`;
/// freed with `LocalFree` on drop.
#[cfg(windows)]
struct OwnerOnlyDacl(*mut std::ffi::c_void);

#[cfg(windows)]
impl OwnerOnlyDacl {
    /// SDDL: a protected DACL (`D:P`) with one ACE granting full access
    /// (`FA`) to the current user's SID only.
    fn new() -> std::io::Result<Self> {
        let sid = current_user_sid_string()?;
        let sddl = format!("D:P(A;;FA;;;{sid})");
        let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `descriptor`
        // is a valid out pointer; the size pointer may be NULL.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

#[cfg(windows)]
impl Drop for OwnerOnlyDacl {
    fn drop(&mut self) {
        // SAFETY: the pointer came from LocalAlloc through the conversion call
        // and is freed exactly once, here.
        unsafe {
            LocalFree(self.0 as HLOCAL);
        }
    }
}

/// The user SID of this process token as `S-1-5-...`.
#[cfg(windows)]
fn current_user_sid_string() -> std::io::Result<String> {
    // SAFETY: every handle and pointer below is checked before use; the token
    // is closed by `TokenHandle` and the SID string freed with `LocalFree`.
    unsafe {
        let mut token: HANDLE = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let token = TokenHandle(token);
        let mut len: u32 = 0;
        GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut len);
        if len == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut buffer = vec![0u8; len as usize];
        let read = GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            len,
            &mut len,
        );
        if read == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation filled `buffer` with a TOKEN_USER whose
        // SID pointer stays inside it for the rest of this block.
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut string_sid: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut string_sid) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut len = 0usize;
        while *string_sid.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(string_sid, len));
        LocalFree(string_sid as HLOCAL);
        Ok(text)
    }
}

/// Owns a process token handle and closes it on drop.
#[cfg(windows)]
struct TokenHandle(HANDLE);

#[cfg(windows)]
impl Drop for TokenHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcessToken and is closed once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
