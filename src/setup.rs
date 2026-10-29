use anyhow::{bail, Context, Result};
use std::{
    fs::{self, File},
    io,
    path::{Component, Path, PathBuf},
};

pub const PLATFORM_TOOLS_URL: &str =
    "https://developer.android.com/tools/releases/platform-tools";
pub const GOOGLE_DRIVER_URL: &str = "https://developer.android.com/studio/run/win-usb";
pub const OEM_DRIVERS_URL: &str = "https://developer.android.com/studio/run/oem-usb";
pub const VC_RUNTIME_URL: &str = "https://aka.ms/vc14/vc_redist.x64.exe";

pub fn open_link(url: &'static str) -> Result<()> {
    let allowed = [
        PLATFORM_TOOLS_URL,
        GOOGLE_DRIVER_URL,
        OEM_DRIVERS_URL,
        VC_RUNTIME_URL,
    ];
    if !allowed.contains(&url) {
        bail!("Unrecognized official download URL");
    }
    webbrowser::open(url).context("Unable to open the browser")?;
    Ok(())
}

/// Imports a Platform Tools ZIP that the user downloaded after accepting Google's terms.
/// Installs into a unique per-user directory without requiring administrator privileges.
pub fn install_platform_tools(zip_path: &Path) -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .context("Could not locate user application-data directory")?
        .join("AndroidBackup")
        .join("tools");
    fs::create_dir_all(&base)?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let folder = base.join(format!("platform-tools-{stamp}"));
    fs::create_dir(&folder)?;

    let outcome = unpack_platform_tools(zip_path, &folder);
    if outcome.is_err() {
        let _ = fs::remove_dir_all(&folder);
    }
    outcome.map(|()| folder.join(adb_filename()))
}

fn adb_filename() -> &'static str {
    if cfg!(windows) {
        "adb.exe"
    } else {
        "adb"
    }
}

fn unpack_platform_tools(zip_path: &Path, directory: &Path) -> Result<()> {
    let file = File::open(zip_path).context("Unable to open ZIP file")?;
    let mut archive = zip::ZipArchive::new(file).context("Invalid Platform Tools ZIP")?;
    if archive.len() > 4000 {
        bail!("Unexpected number of ZIP entries");
    }

    let mut total = 0_u64;
    let mut found_adb = false;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let enclosed = entry.enclosed_name().context("Unsafe ZIP path")?;
        let mut parts = enclosed.components();
        if !matches!(parts.next(), Some(Component::Normal(name)) if name == "platform-tools") {
            bail!("ZIP must contain Google's platform-tools directory");
        }

        let relative: PathBuf = parts.collect();
        if relative.as_os_str().is_empty() {
            continue;
        }
        if !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            bail!("Unsafe path inside ZIP");
        }
        if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
            bail!("Symbolic links are not supported in Platform Tools archives");
        }

        total = total.saturating_add(entry.size());
        if total > 400 * 1024 * 1024 {
            bail!("ZIP contains more than 400 MiB of extracted data");
        }

        let output = directory.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut destination = File::create(&output)?;
        io::copy(&mut entry, &mut destination)?;
        if relative == Path::new(adb_filename()) {
            found_adb = true;
        }
    }

    if !found_adb {
        bail!("The archive contains no {}", adb_filename());
    }
    Ok(())
}

/// On Windows, request explicit UAC consent to stage and install the user-selected
/// driver INF. Windows validates the driver's catalog/signature. This reports only
/// that the installer was launched, not that a compatible device was matched.
#[cfg(windows)]
pub fn install_driver_inf(inf: &Path) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;

    if !inf.is_file() || !inf.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("inf")) {
        bail!("Select an existing .inf driver file");
    }
    let absolute = inf.canonicalize()?;
    let filename = absolute.to_string_lossy();
    if filename.contains('"') {
        bail!("Invalid INF filename");
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }
    let operation = wide(OsStr::new("runas"));
    let program = wide(OsStr::new("pnputil.exe"));
    let arguments = wide(OsStr::new(&format!("/add-driver \"{filename}\" /install")));

    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            program.as_ptr(),
            arguments.as_ptr(),
            ptr::null(),
            0,
        )
    };
    if (result as isize) <= 32 {
        bail!("Windows declined to start driver installer (code {})", result as isize);
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn install_driver_inf(_inf: &Path) -> Result<()> {
    bail!("USB driver installation is only supported on Windows");
}

pub fn open_device_manager() -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("mmc.exe")
            .arg("devmgmt.msc")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .context("Unable to open Windows Device Manager")?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        bail!("Device Manager is only available on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_download_sources_are_https() {
        for url in [
            PLATFORM_TOOLS_URL,
            GOOGLE_DRIVER_URL,
            OEM_DRIVERS_URL,
            VC_RUNTIME_URL,
        ] {
            assert!(url.starts_with("https://"));
        }
    }

    #[test]
    fn known_adb_binary_name() {
        assert!(!adb_filename().is_empty());
    }
}
