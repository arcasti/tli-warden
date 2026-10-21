//! A per-user install with no installer program and no admin rights: the
//! downloaded warden.exe copies itself to %LOCALAPPDATA%\Programs\Warden,
//! adds a Start menu shortcut, a desktop shortcut and an entry in Windows'
//! installed apps, whose Uninstall runs `warden.exe uninstall`.

use super::update;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, RPC_E_CHANGED_MODE};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
use windows::core::{HSTRING, w};

/// The installed apps entry, under HKEY_CURRENT_USER.
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Warden";

/// %LOCALAPPDATA%\Programs\Warden; None when LOCALAPPDATA is not set.
pub fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join("Warden"))
}

/// The Start menu shortcut; None when APPDATA is not set.
fn shortcut_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs\Warden.lnk"))
}

/// The desktop shortcut, in the folder Windows knows as the desktop (which
/// OneDrive may have moved out of %USERPROFILE%).
fn desktop_path() -> Result<PathBuf, String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: the returned string is copied out, then freed with CoTaskMemFree as the API requires.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).map_err(|e| format!("the desktop folder: {}", e.message()))?;
        let dir = path.to_hstring();
        CoTaskMemFree(Some(path.0 as *const _));
        Ok(PathBuf::from(dir.to_os_string()).join("Warden.lnk"))
    }
}

/// Writes the desktop shortcut to `exe`.
pub fn desktop_shortcut(exe: &Path) -> Result<(), String> {
    let link = desktop_path()?;
    shortcut(exe, &link).map_err(|e| format!("{}: {e}", link.display()))
}

/// Whether `current` runs from the install folder.
pub fn installed_here(current: &Path) -> bool {
    install_dir().zip(current.parent()).is_some_and(|(dir, parent)| same_path(&dir, parent))
}

/// The same path, compared canonicalised where they exist and without case.
fn same_path(a: &Path, b: &Path) -> bool {
    let plain = |p: &Path| {
        let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
        p.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(p)
    };
    plain(a) == plain(b)
}

/// Installs `current`: copies it to the install folder (swapping out a
/// warden.exe already there, which may be running), adds the Start menu and
/// desktop shortcuts and the installed apps entry. Returns the installed exe,
/// which the caller starts.
pub fn install(current: &Path) -> Result<PathBuf, String> {
    let dir = install_dir().ok_or("LOCALAPPDATA is not set")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let exe = dir.join(update::ASSET);
    if !same_path(current, &exe) {
        let copy = |to: &Path| std::fs::copy(current, to).map(drop).map_err(|e| format!("{} to {}: {e}", current.display(), to.display()));
        if exe.exists() {
            let new = dir.join(update::NEW);
            copy(&new)?;
            update::swap(&exe, &new)?;
        } else {
            copy(&exe)?;
        }
    }
    let link = shortcut_path().ok_or("APPDATA is not set")?;
    shortcut(&exe, &link).map_err(|e| format!("{}: {e}", link.display()))?;
    // The desktop is a convenience: an install goes ahead without it.
    let _ = desktop_shortcut(&exe);
    register(&exe)?;
    Ok(exe)
}

/// Writes a shell link at `link` that starts `exe`.
fn shortcut(exe: &Path, link: &Path) -> Result<(), String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::Interface;
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // SAFETY: COM is set up on this thread for the calls below (or already
    // was, in another mode, which suits an in-process server too).
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() && init != RPC_E_CHANGED_MODE {
        return Err(init.message());
    }
    let save = || -> windows::core::Result<()> {
        // SAFETY: plain COM calls on interfaces this closure owns, with strings that outlive them.
        unsafe {
            let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            shell_link.SetPath(&HSTRING::from(exe.as_os_str()))?;
            if let Some(dir) = exe.parent() {
                shell_link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
            }
            shell_link.SetDescription(w!("Live advisor for the Tartarus prison board"))?;
            shell_link.cast::<IPersistFile>()?.Save(&HSTRING::from(link.as_os_str()), true)
        }
    };
    let saved = save();
    if init.is_ok() {
        // SAFETY: balances the successful CoInitializeEx above, with the interfaces released.
        unsafe { CoUninitialize() };
    }
    saved.map_err(|e| e.message())
}

/// Writes the installed apps entry for `exe`; called at install and again at
/// start, so an update shows its version.
pub fn register(exe: &Path) -> Result<(), String> {
    use windows::Win32::System::Registry::{HKEY, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};
    let dir = exe.parent().ok_or_else(|| format!("{}: no folder", exe.display()))?;
    let kb = std::fs::metadata(exe).map_err(|e| format!("{}: {e}", exe.display()))?.len().div_ceil(1024);
    let exe_text = exe.display().to_string();
    let texts = [
        ("DisplayName", "Warden".to_string()),
        ("DisplayVersion", env!("CARGO_PKG_VERSION").to_string()),
        ("Publisher", "arcasti".to_string()),
        ("DisplayIcon", format!("{exe_text},0")),
        ("InstallLocation", dir.display().to_string()),
        ("UninstallString", format!("\"{exe_text}\" uninstall")),
        ("URLInfoAbout", "https://github.com/arcasti/tli-warden".to_string()),
    ];
    let numbers = [("NoModify", 1), ("NoRepair", 1), ("EstimatedSize", u32::try_from(kb).unwrap_or(u32::MAX))];
    let failed = |step: &str, e: windows::core::Error| format!(r"HKEY_CURRENT_USER\{UNINSTALL_KEY} {step}: {}", e.message());
    let mut key = HKEY::default();
    // SAFETY: `key` is written by the call and closed below; the strings outlive it.
    unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY), None, None, REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None)
            .ok()
            .map_err(|e| failed("create", e))?;
    }
    let mut result = Ok(());
    for (name, text) in &texts {
        let wide: Vec<u8> = text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        // SAFETY: `key` is open; the data is a nul-terminated UTF-16 string.
        result = result.and_then(|()| unsafe { RegSetValueExW(key, &HSTRING::from(*name), None, REG_SZ, Some(&wide)) }.ok().map_err(|e| failed(name, e)));
    }
    for (name, number) in numbers {
        // SAFETY: `key` is open; the data is a DWORD.
        result = result.and_then(|()| unsafe { RegSetValueExW(key, &HSTRING::from(name), None, REG_DWORD, Some(&number.to_le_bytes())) }.ok().map_err(|e| failed(name, e)));
    }
    // SAFETY: `key` was opened above and is not used after.
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

/// Asks, then removes the Start menu and desktop shortcuts, the installed
/// apps entry and the install folder; the caller exits next. Settings and
/// run logs stay.
pub fn uninstall() -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW};
    // SAFETY: both strings outlive the call and no owner window is given.
    let answer = unsafe {
        MessageBoxW(None, w!("Uninstall Warden? Your settings and run logs in %APPDATA%\\warden are kept."), w!("Warden"), MB_YESNO | MB_ICONQUESTION)
    };
    if answer != IDYES {
        return Ok(());
    }
    for link in [shortcut_path(), desktop_path().ok()].into_iter().flatten() {
        match std::fs::remove_file(&link) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", link.display())),
            _ => {}
        }
    }
    // SAFETY: the key name outlives the call.
    let deleted = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(UNINSTALL_KEY)) };
    if deleted != ERROR_FILE_NOT_FOUND {
        deleted.ok().map_err(|e| format!(r"HKEY_CURRENT_USER\{UNINSTALL_KEY}: {}", e.message()))?;
    }
    let Some(dir) = install_dir().filter(|d| d.exists()) else { return Ok(()) };
    let current = std::env::current_exe().map_err(|e| format!("this program's path: {e}"))?;
    remove_dir(&dir, &current, &std::env::temp_dir())
}

/// Removes the install folder `dir`. A running exe can't be deleted but can
/// be moved, so `current`, when it runs from `dir`, first goes to `temp`
/// (as warden-uninstalled-<pid>.exe), which Windows' temporary file cleanup
/// empties.
fn remove_dir(dir: &Path, current: &Path, temp: &Path) -> Result<(), String> {
    if current.parent().is_some_and(|p| same_path(p, dir)) {
        let parked = temp.join(format!("warden-uninstalled-{}.exe", std::process::id()));
        std::fs::rename(current, &parked).map_err(|e| format!("{} to {}: {e}", current.display(), parked.display()))?;
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_here_paths() {
        let dir = install_dir().unwrap();
        assert!(installed_here(&dir.join(update::ASSET)));
        assert!(installed_here(Path::new(&dir.join(update::ASSET).to_string_lossy().to_uppercase())));
        assert!(!installed_here(&std::env::temp_dir().join(update::ASSET)));
        assert!(!installed_here(&dir.join("sub").join(update::ASSET)));
    }

    #[test]
    fn desktop_path_is_in_an_existing_folder() {
        let link = desktop_path().unwrap();
        assert_eq!(link.file_name().unwrap(), "Warden.lnk");
        assert!(link.parent().unwrap().is_dir());
    }

    #[test]
    fn remove_dir_moves_the_running_exe_out_first() {
        let base = std::env::temp_dir().join(format!("warden-remove-dir-{}", std::process::id()));
        let (dir, temp) = (base.join("Warden"), base.join("temp"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&temp).unwrap();
        // ping.exe stands in for the running warden.exe: copied in and run
        // from there, it holds the folder as a running image does.
        let exe = dir.join(update::ASSET);
        std::fs::copy(r"C:\Windows\System32\PING.EXE", &exe).unwrap();
        std::fs::write(dir.join("warden.old.exe"), b"old").unwrap();
        let mut child = std::process::Command::new(&exe).args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        assert!(std::fs::remove_file(&exe).is_err(), "a running exe can't be deleted");
        let parked = temp.join(format!("warden-uninstalled-{}.exe", std::process::id()));
        let result = remove_dir(&dir, &exe, &temp);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(result, Ok(()));
        assert!(!dir.exists() && parked.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
