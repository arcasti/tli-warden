//! Updates from the GitHub releases: the latest release is checked, its one
//! asset, warden.exe, downloaded and checked against the release's SHA-256,
//! and swapped in for the running exe, which takes effect on the next start.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The latest release, as the GitHub API gives it.
const LATEST: &str = "https://api.github.com/repos/arcasti/tli-warden/releases/latest";

/// The release asset, the only one.
pub(crate) const ASSET: &str = "warden.exe";

/// The name a downloaded exe takes beside the running one until it is swapped in.
pub(crate) const NEW: &str = "warden.new.exe";

/// The name the replaced exe takes until the next start deletes it.
const OLD: &str = "warden.old.exe";

/// A release on GitHub, with its warden.exe asset.
#[derive(Clone)]
pub struct Release {
    pub version: [u32; 3],
    /// The release's web page.
    pub page: String,
    /// warden.exe's download URL.
    pub url: String,
    /// warden.exe's size in bytes.
    pub size: u64,
    pub sha256: [u8; 32],
}

impl Release {
    /// Newer than the running app.
    pub fn is_newer(&self) -> bool {
        version(env!("CARGO_PKG_VERSION")).is_some_and(|running| self.version > running)
    }
}

/// "X.Y.Z" or "vX.Y.Z" as numbers.
pub fn version(text: &str) -> Option<[u32; 3]> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.').map(|p| p.parse::<u32>().ok());
    let v = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(v)
}

/// The release in the latest-release API's JSON; None when the tag, the
/// warden.exe asset, its size or its SHA-256 digest is missing, so nothing
/// is ever downloaded without a digest to check it against.
pub fn parse_release(json: &str) -> Option<Release> {
    let root: serde_json::Value = serde_json::from_str(json).ok()?;
    let asset = root["assets"].as_array()?.iter().find(|a| a["name"] == ASSET)?;
    Some(Release {
        version: version(root["tag_name"].as_str()?)?,
        page: root["html_url"].as_str()?.to_string(),
        url: asset["browser_download_url"].as_str()?.to_string(),
        size: asset["size"].as_u64()?,
        sha256: hex32(asset["digest"].as_str()?.strip_prefix("sha256:")?)?,
    })
}

/// 64 hex digits as 32 bytes.
fn hex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut bytes = [0; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(bytes)
}

/// Ok when `bytes` are the release's asset: its size and SHA-256.
fn verified(bytes: &[u8], release: &Release) -> Result<(), String> {
    if bytes.len() as u64 != release.size {
        return Err(format!("{}: {} bytes downloaded, the release has {}", release.url, bytes.len(), release.size));
    }
    if Sha256::digest(bytes)[..] != release.sha256 {
        return Err(format!("{}: the SHA-256 differs from the release's", release.url));
    }
    Ok(())
}

/// The latest release when it is newer than the running app.
pub fn check() -> Result<Option<Release>, String> {
    let json = latest_json()?;
    let release = parse_release(&json).ok_or_else(|| format!("{LATEST}: no {ASSET} with a SHA-256 digest in the latest release"))?;
    Ok(release.is_newer().then_some(release))
}

/// The latest-release API's JSON.
fn latest_json() -> Result<String, String> {
    String::from_utf8(get(LATEST, "application/vnd.github+json")?).map_err(|e| format!("{LATEST}: {e}"))
}

/// Downloads the release's warden.exe into `dir` as warden.new.exe, once it
/// checks out against the release's size and SHA-256.
pub fn download(release: &Release, dir: &Path) -> Result<PathBuf, String> {
    let bytes = get(&release.url, "application/octet-stream")?;
    verified(&bytes, release)?;
    let path = dir.join(NEW);
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// The body of a GET of `url`. HttpClient follows redirects, such as the
/// release asset's to objects.githubusercontent.com.
fn get(url: &str, accept: &str) -> Result<Vec<u8>, String> {
    use windows::Foundation::Uri;
    use windows::Storage::Streams::DataReader;
    use windows::Web::Http::HttpClient;
    use windows::core::{HSTRING, h};
    let fetch = || -> windows::core::Result<Result<Vec<u8>, String>> {
        let client = HttpClient::new()?;
        let headers = client.DefaultRequestHeaders()?;
        headers.TryAppendWithoutValidation(h!("User-Agent"), &HSTRING::from(concat!("warden/", env!("CARGO_PKG_VERSION"))))?;
        headers.TryAppendWithoutValidation(h!("Accept"), &HSTRING::from(accept))?;
        let response = client.GetAsync(&Uri::CreateUri(&HSTRING::from(url))?)?.join()?;
        if !response.IsSuccessStatusCode()? {
            return Ok(Err(format!("HTTP {}", response.StatusCode()?.0)));
        }
        let buffer = response.Content()?.ReadAsBufferAsync()?.join()?;
        let mut bytes = vec![0; buffer.Length()? as usize];
        DataReader::FromBuffer(&buffer)?.ReadBytes(&mut bytes)?;
        Ok(Ok(bytes))
    };
    fetch().map_err(|e| e.message()).and_then(|r| r).map_err(|e| format!("{url}: {e}"))
}

/// Puts `new` in place of `current`. Windows renames a running exe but does
/// not overwrite it, so `current` becomes warden.old.exe beside it first; when
/// `new` cannot take its place, the old one goes back.
pub fn swap(current: &Path, new: &Path) -> Result<(), String> {
    let old = current.with_file_name(OLD);
    if old.exists() {
        std::fs::remove_file(&old).map_err(|e| format!("{}: {e}", old.display()))?;
    }
    std::fs::rename(current, &old).map_err(|e| format!("{} to {}: {e}", current.display(), old.display()))?;
    if let Err(e) = std::fs::rename(new, current) {
        let _ = std::fs::rename(&old, current);
        return Err(format!("{} to {}: {e}", new.display(), current.display()));
    }
    Ok(())
}

/// Deletes the warden.old.exe a swap left beside `current`, if any.
pub fn clean_old(current: &Path) {
    let _ = std::fs::remove_file(current.with_file_name(OLD));
}

/// The environment variable [`restart`] passes this process's id in.
const WAIT_PID: &str = "WARDEN_WAIT_PID";

/// Starts `current` on its own, with no arguments; the caller then exits.
/// The new process waits for this one in [`wait_for_previous`].
pub fn restart(current: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    std::process::Command::new(current)
        .env(WAIT_PID, std::process::id().to_string())
        .creation_flags(DETACHED_PROCESS)
        .spawn()
        .map(drop)
        .map_err(|e| format!("{}: {e}", current.display()))
}

/// Waits up to 5 s for the Warden that started this one with [`restart`] to
/// exit, so its hotkeys and its exe file are free; returns at once on a
/// normal start.
pub fn wait_for_previous() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    let Some(pid) = std::env::var(WAIT_PID).ok().and_then(|p| p.parse::<u32>().ok()) else { return };
    // SAFETY: the handle is only waited on and then closed.
    unsafe {
        if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(process, 5000);
            let _ = CloseHandle(process);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"{"tag_name":"v0.2.1","html_url":"https://github.com/arcasti/tli-warden/releases/tag/v0.2.1","assets":[{"name":"warden.exe","size":9116672,"digest":"sha256:64e0254243ac9da113bb5a3934cf2382fcc9abfedc80591f04484a1aa6bd0aaf","browser_download_url":"https://github.com/arcasti/tli-warden/releases/download/v0.2.1/warden.exe"}]}"#;

    /// SHA-256 of b"warden" (`printf warden | sha256sum`).
    const WARDEN_SHA: &str = "8bdb247a2a76e166450e193f185f0deeceedee6550589de2cd964e7ce36c9de6";

    #[test]
    fn parse_release_json() {
        let r = parse_release(JSON).unwrap();
        assert_eq!((r.version, r.size), ([0, 2, 1], 9116672));
        assert_eq!(r.page, "https://github.com/arcasti/tli-warden/releases/tag/v0.2.1");
        assert_eq!(r.url, "https://github.com/arcasti/tli-warden/releases/download/v0.2.1/warden.exe");
        assert_eq!((r.sha256[0], r.sha256[31]), (0x64, 0xaf));
        assert!(parse_release(&JSON.replace("\"warden.exe\"", "\"warden-cli.exe\"")).is_none());
        assert!(parse_release(&JSON.replace("\"digest\"", "\"other\"")).is_none());
        assert!(parse_release(&JSON.replace("sha256:64e0", "sha256:")).is_none());
        assert!(parse_release(&JSON.replace("v0.2.1\",\"html", "latest\",\"html")).is_none());
        assert!(parse_release(&JSON.replace("v0.2.1\",\"html", "0.2.1\",\"html")).is_some());
    }

    #[test]
    fn version_order() {
        assert_eq!(version("v0.2.1"), Some([0, 2, 1]));
        assert_eq!((version("0.2"), version("0.2.1.4"), version("0.2.x")), (None, None, None));
        assert!(version("0.2.1") < version("0.3.0"));
        assert!(version("0.10.0") > version("0.9.9"));
        let mut r = parse_release(JSON).unwrap();
        r.version = version(env!("CARGO_PKG_VERSION")).unwrap();
        assert!(!r.is_newer());
        r.version[2] += 1;
        assert!(r.is_newer());
    }

    #[test]
    fn verified_bytes() {
        let mut r = parse_release(JSON).unwrap();
        r.size = 6;
        r.sha256 = hex32(WARDEN_SHA).unwrap();
        assert!(verified(b"warden", &r).is_ok());
        assert!(verified(b"wardem", &r).is_err());
        assert!(verified(b"warden!", &r).is_err());
    }

    /// A fresh folder under the temp folder.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("warden-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn swap_and_clean_old() {
        let dir = temp("swap");
        let (current, new, old) = (dir.join("warden.exe"), dir.join(NEW), dir.join(OLD));
        std::fs::write(&current, "current").unwrap();
        std::fs::write(&new, "new").unwrap();
        std::fs::write(&old, "stale").unwrap();
        swap(&current, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&current).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "current");
        assert!(!new.exists());
        assert!(swap(&current, &new).is_err());
        assert_eq!(std::fs::read_to_string(&current).unwrap(), "new");
        clean_old(&current);
        assert!(!old.exists());
        clean_old(&current);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The live API, for manual runs: `cargo test --lib live_latest_release -- --ignored`.
    /// Also downloads the asset, through GitHub's redirect, and checks it.
    #[test]
    #[ignore]
    fn live_latest_release() {
        check().unwrap();
        let release = parse_release(&latest_json().unwrap()).unwrap();
        let dir = temp("download");
        let path = download(&release, &dir).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), release.size);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
