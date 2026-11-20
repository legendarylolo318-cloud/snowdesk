//! Start-with-desktop toggle. Linux: XDG autostart entry. Windows: HKCU Run key.
//! (On Hyprland you can also use `exec-once = snowdesk --hidden`, see README.)

pub fn set(enabled: bool) {
    if let Err(e) = imp::set(enabled) {
        tracing::warn!("autostart: {e}");
    }
}

fn exe_path() -> String {
    std::env::var("APPIMAGE")
        .ok()
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| "snowdesk".into())
}

/// XDG autostart entry. The Exec path is quoted and escaped per the
/// Desktop Entry spec, so paths with spaces, quotes or `$` work.
#[cfg(unix)]
fn desktop_entry(exe: &str) -> String {
    let mut q = String::new();
    for c in exe.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                q.push('\\');
                q.push(c);
            }
            '%' => q.push_str("%%"),
            '\n' | '\r' => {}
            c => q.push(c),
        }
    }
    // Backslashes are escaped once more because the value is a string.
    let q = q.replace('\\', "\\\\");
    format!(
        "[Desktop Entry]\nType=Application\nName=snowdesk\nExec=\"{q}\" --hidden\nX-GNOME-Autostart-enabled=true\n"
    )
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn exec_line_is_quoted_and_escaped() {
        let e = super::desktop_entry("/home/me/My Apps/snow$desk 100%.AppImage");
        assert!(
            e.contains(r#"Exec="/home/me/My Apps/snow\\$desk 100%%.AppImage" --hidden"#),
            "{e}"
        );
    }
}

#[cfg(unix)]
mod imp {
    use super::desktop_entry;
    pub fn set(enabled: bool) -> std::io::Result<()> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
            })
            .ok_or_else(|| std::io::Error::other("no config dir"))?;
        let dir = base.join("autostart");
        let file = dir.join("snowdesk.desktop");
        if enabled {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(file, desktop_entry(&super::exe_path()))
        } else {
            match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
        RegOpenKeyExW, RegSetValueExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn set(enabled: bool) -> std::io::Result<()> {
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            let sub = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
            if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_SET_VALUE, &mut key) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let name = wide("snowdesk");
            if enabled {
                let val = wide(&format!("\"{}\" --hidden", super::exe_path()));
                RegSetValueExW(
                    key,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    val.as_ptr() as *const u8,
                    (val.len() * 2) as u32,
                );
            } else {
                RegDeleteValueW(key, name.as_ptr());
            }
            RegCloseKey(key);
        }
        Ok(())
    }
}
