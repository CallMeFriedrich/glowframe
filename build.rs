//! Embeds the app icon, version info (from Cargo.toml) and the application
//! manifest into the Windows executable (SPEC §9).

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/glowframe.manifest");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = env("CARGO_PKG_VERSION");
    let parts: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let (major, minor, patch) = (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    );
    let root = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let icon = root.join("assets").join("icon.ico");
    let manifest = root.join("assets").join("glowframe.manifest");
    let description = env("CARGO_PKG_DESCRIPTION");

    // Resource ID 1 is the app icon (also used for the tray icon).
    let rc = format!(
        r#"#include <winver.h>
1 ICON "{icon}"
1 24 "{manifest}"
VS_VERSION_INFO VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Glowframe"
      VALUE "FileDescription", "Glowframe"
      VALUE "Comments", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "Glowframe"
      VALUE "OriginalFilename", "Glowframe.exe"
      VALUE "ProductName", "Glowframe"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = rc_path(&icon),
        manifest = rc_path(&manifest),
    );
    let out = PathBuf::from(env("OUT_DIR")).join("glowframe.rc");
    if let Err(e) = std::fs::write(&out, rc) {
        panic!("cannot write {}: {e}", out.display());
    }
    if let Err(e) = embed_resource::compile(&out, embed_resource::NONE).manifest_required() {
        panic!("resource compilation failed: {e}");
    }
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

/// Paths in .rc files need escaped backslashes.
fn rc_path(p: &std::path::Path) -> String {
    p.display().to_string().replace('\\', "\\\\")
}
