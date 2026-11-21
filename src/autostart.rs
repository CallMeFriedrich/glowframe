//! Start with Windows (SPEC §7 v1.5): a value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Per user, no admin.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, w};

const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: windows::core::PCWSTR = w!("Glowframe");

/// Command line stored in the Run key for `exe`.
pub fn command_for(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

fn current_command() -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: buffer and size describe a valid, writable u16 buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Makes the registry match `enabled` for the running exe. Also rewrites
/// a stale path (exe moved, portable copy updated).
pub fn apply(enabled: bool) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "autostart: exe path unknown");
            return;
        }
    };
    let wanted = command_for(&exe);
    let current = current_command();
    if enabled {
        if current.as_deref() == Some(wanted.as_str()) {
            return;
        }
        let data: Vec<u16> = wanted.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: data is a NUL-terminated UTF-16 string of the given byte size.
        let r = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                &HSTRING::from("Glowframe"),
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * 2) as u32,
            )
        };
        if r == ERROR_SUCCESS {
            tracing::info!(command = %wanted, "autostart enabled");
        } else {
            tracing::warn!(error = ?r, "autostart: registry write failed");
        }
    } else if current.is_some() {
        // SAFETY: plain registry call with static strings.
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE) };
        if r == ERROR_SUCCESS {
            tracing::info!("autostart disabled");
        } else {
            tracing::warn!(error = ?r, "autostart: registry delete failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_quoted() {
        let c = command_for(std::path::Path::new(
            r"C:\Program Files\Glowframe\Glowframe.exe",
        ));
        assert_eq!(c, r#""C:\Program Files\Glowframe\Glowframe.exe""#);
    }
}
