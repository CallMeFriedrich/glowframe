//! Backgrounds of the animation monitors (SPEC §5.1 v1.5): Windows wallpaper
//! or a custom image (>= 2560x1440), picked via the Windows file dialog,
//! copied into the data folder and decoded off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETDESKWALLPAPER, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{HSTRING, w};

use crate::cover::Image;
use crate::cover::wic::Wic;

/// Minimum size of a custom background (either orientation).
pub const MIN_SIZE: (u32, u32) = (2560, 1440);
/// Backgrounds are decoded at most this large (4K).
const MAX_DECODE: (u32, u32) = (3840, 2400);
const MAX_FILE_BYTES: u64 = 80 * 1024 * 1024;
const FILE_STEM: &str = "background";

/// Why a picked image was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    TooSmall { width: u32, height: u32 },
    Unreadable,
}

pub fn big_enough(w: u32, h: u32) -> bool {
    let (long, short) = (w.max(h), w.min(h));
    long >= MIN_SIZE.0 && short >= MIN_SIZE.1
}

/// Path of the current Windows desktop wallpaper, if any.
pub fn wallpaper_path() -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    // SAFETY: the buffer is writable and its length is passed in characters.
    unsafe {
        SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr().cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
    (len > 0 && path.is_file()).then_some(path)
}

/// Opens the Windows file dialog for images. Runs on the UI thread.
pub fn pick_file(title: &str, filter_name: &str) -> Option<PathBuf> {
    // SAFETY: COM dialog used and released on this thread; strings outlive
    // the calls; the returned path is freed with CoTaskMemFree.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let name = HSTRING::from(filter_name);
        let filters = [COMDLG_FILTERSPEC {
            pszName: windows::core::PCWSTR(name.as_ptr()),
            pszSpec: w!("*.jpg;*.jpeg;*.png;*.bmp;*.webp;*.jxr;*.tif;*.tiff"),
        }];
        dialog.SetFileTypes(&filters).ok()?;
        dialog.SetTitle(&HSTRING::from(title)).ok()?;
        let opts = dialog.GetOptions().ok()?;
        dialog
            .SetOptions(opts | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)
            .ok()?;
        // Cancel returns an error: no selection.
        dialog.Show(None).ok()?;
        let item = dialog.GetResult().ok()?;
        let raw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = raw.to_string().ok();
        CoTaskMemFree(Some(raw.0 as *const _));
        path.map(PathBuf::from)
    }
}

fn with_wic<T>(f: impl FnOnce(&Wic) -> Option<T>) -> Option<T> {
    // SAFETY: COM init for the calling worker thread (idempotent).
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let wic = Wic::new().ok()?;
    f(&wic)
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    let len = std::fs::metadata(path).ok()?.len();
    (len <= MAX_FILE_BYTES)
        .then(|| std::fs::read(path).ok())
        .flatten()
}

/// Checks the size of a picked image and copies it into `config_dir` as
/// `background.<ext>` (older copies are removed). Returns the file name.
/// Call from a worker thread.
pub fn import(src: &Path, config_dir: &Path) -> Result<String, ImportError> {
    let bytes = read_file(src).ok_or(ImportError::Unreadable)?;
    let (width, height) = with_wic(|w| w.size(&bytes).ok()).ok_or(ImportError::Unreadable)?;
    if !big_enough(width, height) {
        return Err(ImportError::TooSmall { width, height });
    }
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("img")
        .to_ascii_lowercase();
    let name = format!("{FILE_STEM}.{ext}");
    std::fs::create_dir_all(config_dir).map_err(|_| ImportError::Unreadable)?;
    if let Ok(entries) = std::fs::read_dir(config_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.file_stem().and_then(|s| s.to_str()) == Some(FILE_STEM) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    std::fs::write(config_dir.join(&name), &bytes).map_err(|_| ImportError::Unreadable)?;
    tracing::info!(width, height, file = %name, "custom background imported");
    Ok(name)
}

/// Decodes a background image (scaled to at most 4K). Worker thread.
pub fn load(path: &Path) -> Option<Arc<Image>> {
    let bytes = read_file(path)?;
    match with_wic(|w| w.decode_fit(&bytes, MAX_DECODE.0, MAX_DECODE.1).ok()) {
        Some(img) => {
            tracing::info!(?img, path = %path.display(), "background loaded");
            Some(Arc::new(img))
        }
        None => {
            tracing::warn!(path = %path.display(), "background could not be decoded");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_size_in_both_orientations() {
        assert!(big_enough(2560, 1440));
        assert!(big_enough(3840, 2160));
        assert!(big_enough(1440, 2560));
        assert!(!big_enough(1920, 1080));
        assert!(!big_enough(2560, 1080));
    }
}
