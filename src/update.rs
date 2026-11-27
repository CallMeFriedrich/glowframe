//! Self-update from GitHub releases (SPEC §6.5 v1.5): check, download,
//! verify SHA-256 against the API digest, then hand over to the installer
//! (installed copy) or swap the exe (portable copy) and restart.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;

const LATEST_URL: &str = "https://api.github.com/repos/CallMeFriedrich/glowframe/releases/latest";
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Hide the console window of helper processes.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub type Version = (u32, u32, u32);

/// `v1.2.3` / `1.2.3` -> (1, 2, 3).
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u32>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

pub fn current_version() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or((0, 0, 0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// Lower-case hex SHA-256.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    pub setup: Option<Asset>,
    pub portable: Option<Asset>,
}

impl Release {
    pub fn version_text(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }

    pub fn asset(&self, portable: bool) -> Option<&Asset> {
        if portable {
            self.portable.as_ref()
        } else {
            self.setup.as_ref()
        }
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

/// Parses the GitHub "latest release" JSON. Assets without a SHA-256 digest
/// or not served from github.com are ignored (never installed unverified).
pub fn parse_release(json: &str) -> Option<Release> {
    let r: ApiRelease = serde_json::from_str(json).ok()?;
    if r.draft || r.prerelease {
        return None;
    }
    let version = parse_version(&r.tag_name)?;
    let pick = |suffix: &str| {
        r.assets.iter().find_map(|a| {
            let sha = a
                .digest
                .as_deref()?
                .strip_prefix("sha256:")?
                .to_ascii_lowercase();
            let ok = a.name.starts_with("Glowframe-")
                && a.name.ends_with(suffix)
                && a.browser_download_url
                    .starts_with("https://github.com/CallMeFriedrich/glowframe/releases/download/")
                && sha.len() == 64
                && sha.chars().all(|c| c.is_ascii_hexdigit());
            ok.then(|| Asset {
                name: a.name.clone(),
                url: a.browser_download_url.clone(),
                sha256: sha,
            })
        })
    };
    Some(Release {
        version,
        tag: r.tag_name.clone(),
        setup: pick("-setup.exe"),
        portable: pick("-portable.zip"),
    })
}

fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(format!(
            "Glowframe/{} (https://github.com/CallMeFriedrich/glowframe)",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .into()
}

/// Asks GitHub for the latest release. `Ok(None)` = no newer version.
pub fn check() -> Result<Option<Release>, String> {
    let mut resp = agent()
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(MAX_JSON_BYTES)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let release = parse_release(&text).ok_or("invalid release data")?;
    Ok((release.version > current_version()).then_some(release))
}

/// Lower-case hex SHA-256 via Windows CNG.
pub fn sha256_hex(data: &[u8]) -> Option<String> {
    use windows::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
    let mut out = [0u8; 32];
    // SAFETY: pseudo handle for SHA-256, valid input and 32-byte output.
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) };
    status
        .is_ok()
        .then(|| out.iter().map(|b| format!("{b:02x}")).collect())
}

fn download(asset: &Asset, dir: &Path) -> Result<PathBuf, String> {
    let mut resp = agent().get(&asset.url).call().map_err(|e| e.to_string())?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(MAX_PACKAGE_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let hash = sha256_hex(&bytes).ok_or("hashing failed")?;
    if hash != asset.sha256 {
        return Err(format!("checksum mismatch for {}", asset.name));
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // Plain file name only (asset names come from our own release).
    let name = Path::new(&asset.name).file_name().ok_or("bad asset name")?;
    let path = dir.join(name);
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// PowerShell single-quoted string literal.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Starts a hidden helper that waits for this process to exit and then
/// runs `then` (a PowerShell command).
fn after_exit(then: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let script = format!(
        "Wait-Process -Id {} -Timeout 30 -ErrorAction SilentlyContinue; {then}",
        std::process::id()
    );
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
        ])
        .arg(script)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Downloads and prepares the update. On success the caller must quit the
/// app right away; the helper then installs and restarts Glowframe.
pub fn install(release: &Release, portable: bool) -> Result<(), String> {
    let asset = release
        .asset(portable)
        .ok_or("no verified package for this version")?;
    let dir = std::env::temp_dir().join(format!("Glowframe-update-{}", release.version_text()));
    let file = download(asset, &dir)?;
    tracing::info!(file = %file.display(), "update downloaded and verified");
    if portable {
        install_portable(&file, &dir)
    } else {
        after_exit(&format!(
            "Start-Process -FilePath {} -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/relaunch=1'",
            ps_quote(&file.display().to_string())
        ))
    }
}

/// Portable copy: unpack the new exe, swap it in (a running exe may be
/// renamed), restart after exit. The `.old` file is removed on next start.
fn install_portable(zip: &Path, dir: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let unpack = dir.join("unpacked");
    let _ = std::fs::remove_dir_all(&unpack);
    std::fs::create_dir_all(&unpack).map_err(|e| e.to_string())?;
    let tar = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("System32")
        .join("tar.exe");
    let status = Command::new(tar)
        .arg("-xf")
        .arg(zip)
        .arg("-C")
        .arg(&unpack)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    let new_exe = unpack.join("Glowframe.exe");
    if !status.success() || !new_exe.is_file() {
        return Err("could not unpack the update".into());
    }
    let old = exe.with_extension("exe.old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| e.to_string())?;
    if let Err(e) = std::fs::copy(&new_exe, &exe) {
        // Roll back so the app keeps working.
        let _ = std::fs::rename(&old, &exe);
        return Err(e.to_string());
    }
    after_exit(&format!(
        "Start-Process -FilePath {} -ArgumentList '--tray'",
        ps_quote(&exe.display().to_string())
    ))
}

/// Removes the previous exe left behind by a portable update.
pub fn cleanup_old_exe() {
    if let Ok(exe) = std::env::current_exe() {
        let old = exe.with_extension("exe.old");
        if old.is_file() {
            match std::fs::remove_file(&old) {
                Ok(()) => tracing::info!("removed previous exe after update"),
                Err(e) => tracing::debug!(error = %e, "previous exe not removable yet"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "tag_name": "v0.2.0", "draft": false, "prerelease": false,
        "assets": [
            {"name": "Glowframe-0.2.0-portable.zip",
             "browser_download_url": "https://github.com/CallMeFriedrich/glowframe/releases/download/v0.2.0/Glowframe-0.2.0-portable.zip",
             "digest": "sha256:E61570AA6DC449431EBADDF285ABE07AD72AF3819465BEEE3DDE8DF37F0307D1"},
            {"name": "Glowframe-0.2.0-setup.exe",
             "browser_download_url": "https://github.com/CallMeFriedrich/glowframe/releases/download/v0.2.0/Glowframe-0.2.0-setup.exe",
             "digest": null}
        ]}"#;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.1.5"), Some((0, 1, 5)));
        assert_eq!(parse_version("1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("x"), None);
        assert!((0, 1, 10) > (0, 1, 9));
        assert!(current_version() >= (0, 1, 5));
    }

    #[test]
    fn release_parsing_requires_digest_and_origin() {
        let r = parse_release(SAMPLE).unwrap_or_else(|| panic!("parse failed"));
        assert_eq!(r.version, (0, 2, 0));
        assert_eq!(r.version_text(), "0.2.0");
        let zip = r.portable.as_ref().map(|a| a.sha256.as_str());
        assert_eq!(
            zip,
            Some("e61570aa6dc449431ebaddf285abe07ad72af3819465beee3dde8df37f0307d1")
        );
        // Setup without digest is not offered.
        assert!(r.setup.is_none());
        assert!(r.asset(false).is_none() && r.asset(true).is_some());
        // Foreign download URLs are rejected.
        let evil = SAMPLE.replace("https://github.com/CallMeFriedrich", "https://evil.example");
        assert!(parse_release(&evil).is_some_and(|r| r.portable.is_none()));
        // Pre-releases are ignored.
        assert!(
            parse_release(&SAMPLE.replace("\"prerelease\": false", "\"prerelease\": true"))
                .is_none()
        );
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(b"abc").as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }

    #[test]
    fn powershell_quoting() {
        assert_eq!(
            ps_quote(r"C:\Users\O'Brien\x.exe"),
            r"'C:\Users\O''Brien\x.exe'"
        );
    }
}
