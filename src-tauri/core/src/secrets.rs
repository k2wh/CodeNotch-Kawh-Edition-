//! Reading secrets that apps stash in the system's credential store.
//!
//! On Windows Claude Code normally writes `%USERPROFILE%\.claude\.credentials.json`,
//! but installs that opt into OS-backed storage keep the blob in the generic
//! credential store instead. We read it with `CredReadW`, which needs no
//! elevation for the current user's own credentials.
//!
//! On macOS the store is where Claude Code keeps it: a generic password in
//! the login keychain, as the original CodeNotch reads it too. The first read
//! asks the user to allow it; "Always Allow" lasts until the app changes.

/// Fetch a generic credential's blob as a UTF-8 string.
///
/// Returns `None` when the credential does not exist, is not readable, or the
/// blob is not valid UTF-8. Never panics and never writes.
#[cfg(windows)]
pub fn read_generic_credential(target: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();

    // SAFETY: `wide` is NUL-terminated and outlives the call; on success the
    // returned buffer is handed straight back to CredFree.
    unsafe {
        if CredReadW(PCWSTR(wide.as_ptr()), CRED_TYPE_GENERIC, None, &mut ptr).is_err() {
            return None;
        }
        if ptr.is_null() {
            return None;
        }
        let cred = &*ptr;
        let bytes = if cred.CredentialBlob.is_null() || cred.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
                .to_vec()
        };
        CredFree(ptr as *const _);

        String::from_utf8(bytes).ok()
    }
}

/// Fetch a generic password's data from the keychain, by service name.
///
/// Returns `None` when there is no such item, the user declined to allow the
/// read, or the data is not valid UTF-8.
#[cfg(target_os = "macos")]
pub fn read_generic_credential(target: &str) -> Option<String> {
    use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};

    let mut search = ItemSearchOptions::new();
    search
        .class(ItemClass::generic_password())
        .service(target)
        .load_data(true)
        .limit(1);
    match search.search().ok()?.into_iter().next()? {
        SearchResult::Data(data) => String::from_utf8(data).ok(),
        _ => None,
    }
}

/// Elsewhere there is no credential store to read; adapters use their files.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn read_generic_credential(_target: &str) -> Option<String> {
    None
}

/// Credential Manager targets Claude Code is known to use, most specific first.
pub const CLAUDE_CREDENTIAL_TARGETS: &[&str] = &[
    "Claude Code-credentials",
    "Claude Code",
    "claude-code:credentials",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_credentials_are_none_not_a_panic() {
        // The point of the test on non-Windows hosts is that the stub compiles
        // and is safe to call; on Windows it exercises the real miss path.
        assert!(read_generic_credential("codenotch-definitely-not-a-real-target").is_none());
    }

    #[test]
    fn claude_targets_are_ordered_most_specific_first() {
        assert_eq!(CLAUDE_CREDENTIAL_TARGETS[0], "Claude Code-credentials");
    }
}
