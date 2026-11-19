//! Finds the root Spotify process (the one whose parent is not Spotify).

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

pub fn spotify_root_pid() -> Option<u32> {
    let procs = list_processes();
    root_of(&procs, "spotify.exe")
}

/// (pid, parent pid, lowercase exe name)
fn list_processes() -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    // SAFETY: snapshot handle is closed at the end; entry size is set.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut entry).is_ok();
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
            out.push((entry.th32ProcessID, entry.th32ParentProcessID, name));
            ok = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn root_of(procs: &[(u32, u32, String)], exe: &str) -> Option<u32> {
    let matching: Vec<&(u32, u32, String)> = procs.iter().filter(|p| p.2 == exe).collect();
    matching
        .iter()
        .find(|p| !matching.iter().any(|q| q.0 == p.1))
        .or(matching.first())
        .map(|p| p.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_root_process() {
        let procs = vec![
            (10, 1, "explorer.exe".to_string()),
            (20, 10, "spotify.exe".to_string()),
            (21, 20, "spotify.exe".to_string()),
            (22, 20, "spotify.exe".to_string()),
        ];
        assert_eq!(root_of(&procs, "spotify.exe"), Some(20));
        assert_eq!(root_of(&procs, "chrome.exe"), None);
    }
}
