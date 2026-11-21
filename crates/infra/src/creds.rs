//! Saved passwords in the Windows Credential Manager («Диспетчер учётных данных»,
//! generic credentials named `Quokka:<url>`), encrypted by Windows for the current
//! user. Quokka's own settings file never holds a password.

#[cfg(windows)]
mod imp {
    use std::ptr;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND, FILETIME};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn save(target: &str, user: &str, secret: &str) -> Result<(), String> {
        let target = wide(target);
        let user = wide(user);
        let blob: Vec<u8> = secret.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let cred = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_ptr() as *mut u16,
            Comment: ptr::null_mut(),
            LastWritten: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: ptr::null_mut(),
            UserName: user.as_ptr() as *mut u16,
        };
        if unsafe { CredWriteW(&cred, 0) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }

    pub fn load(target: &str) -> Option<String> {
        let target = wide(target);
        let mut out: *mut CREDENTIALW = ptr::null_mut();
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut out) } == 0 || out.is_null() {
            return None;
        }
        let secret = unsafe {
            let c = &*out;
            let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
            let units: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
            String::from_utf16(&units).ok()
        };
        unsafe { CredFree(out as *const _) };
        secret
    }

    pub fn delete(target: &str) -> Result<(), String> {
        let target = wide(target);
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 && unsafe { GetLastError() } != ERROR_NOT_FOUND {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn save(_target: &str, _user: &str, _secret: &str) -> Result<(), String> {
        Err("saving passwords is only supported on Windows".into())
    }
    pub fn load(_target: &str) -> Option<String> {
        None
    }
    pub fn delete(_target: &str) -> Result<(), String> {
        Ok(())
    }
}

/// Credential name of a server login, e.g. `Quokka:sftp://bob@example.com`.
pub fn target(url: &str) -> String {
    format!("Quokka:{url}")
}

/// Remember the password (or key passphrase) for `url`.
pub fn save(url: &str, user: &str, secret: &str) -> Result<(), String> {
    imp::save(&target(url), user, secret)
}

pub fn load(url: &str) -> Option<String> {
    imp::load(&target(url))
}

/// Forget it; not having one is fine.
pub fn delete(url: &str) -> Result<(), String> {
    imp::delete(&target(url))
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn save_load_delete() {
        let url = format!("sftp://test@quokka-unit-test-{}", std::process::id());
        super::save(&url, "test", "пароль 🔑").unwrap();
        assert_eq!(super::load(&url).as_deref(), Some("пароль 🔑"));
        super::save(&url, "test", "new").unwrap();
        assert_eq!(super::load(&url).as_deref(), Some("new"));
        super::delete(&url).unwrap();
        assert_eq!(super::load(&url), None);
        super::delete(&url).unwrap();
    }
}
