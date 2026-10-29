#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod windows_launcher {
    use std::{
        ffi::OsStr,
        fs::{self, OpenOptions},
        io::Write,
        mem,
        os::windows::ffi::OsStrExt,
        path::{Path, PathBuf},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError},
        System::Threading::{GetExitCodeProcess, WaitForSingleObject, WAIT_OBJECT_0},
        UI::{
            Shell::{ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS},
            WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
        },
    };

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    fn log_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join("AndroidBackup").join("Logs").join("launcher.log")
    }

    fn record(message: &str) {
        let path = log_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(path) {
            let _ = writeln!(file, "{message}");
        }
    }

    fn error(message: &str) {
        record(message);
        let title = wide(OsStr::new("Android Backup - Startup error"));
        let body = wide(OsStr::new(message));
        unsafe {
            MessageBoxW(ptr::null_mut(), body.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
        }
    }

    fn check_installation(folder: &Path) -> Result<PathBuf, String> {
        let app = folder.join("App");
        let exe = app.join("AndroidBackup.exe");
        let required = [
            exe.clone(),
            app.join("AndroidBackup.runtimeconfig.json"),
            app.join("AndroidBackup.pri"),
            app.join("Microsoft.ui.xaml.dll"),
            app.join("Assets").join("icon.ico"),
            app.join("backend").join("backup-android-core.exe"),
        ];
        let missing: Vec<_> = required
            .iter()
            .filter(|file| !file.is_file())
            .map(|file| file.strip_prefix(folder).unwrap_or(file).display().to_string())
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "Installation is incomplete. Missing:\n{}\n\nExtract the complete AndroidBackup folder, not only the launcher.\n\nLog: {}",
                missing.join("\n"),
                log_path().display()
            ));
        }
        Ok(exe)
    }

    pub fn run() {
        let root = match std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        {
            Some(root) => root,
            None => {
                error("Could not locate the Android Backup installation directory.");
                return;
            }
        };

        if std::env::args_os().any(|arg| arg == "--check-install") {
            match check_installation(&root) {
                Ok(_) => record("Installation check passed"),
                Err(message) => error(&message),
            }
            return;
        }

        let exe = match check_installation(&root) {
            Ok(exe) => exe,
            Err(message) => {
                error(&message);
                return;
            }
        };
        let directory = exe.parent().expect("validated executable has directory");
        let verb = wide(OsStr::new("runas"));
        let binary = wide(exe.as_os_str());
        let cwd = wide(directory.as_os_str());

        // Windows owns the UAC prompt. Nothing here bypasses elevation.
        let mut info: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };
        info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = binary.as_ptr();
        info.lpDirectory = cwd.as_ptr();
        info.nShow = 1;

        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let code = unsafe { GetLastError() };
            if code != 1223 {
                error(&format!(
                    "Windows could not start WinUI 3 (error {code}).\n\nEnsure the application was fully extracted.\nStartup log: {}",
                    log_path().display()
                ));
            }
            return;
        }

        record("WinUI 3 launch request accepted by Windows");
        if !info.hProcess.is_null() {
            // An early exit commonly indicates a missing dependency or XAML
            // startup failure, rather than a successful WinUI application launch.
            let result = unsafe { WaitForSingleObject(info.hProcess, 4000) };
            if result == WAIT_OBJECT_0 {
                let mut status = 0u32;
                unsafe { GetExitCodeProcess(info.hProcess, &mut status) };
                error(&format!(
                    "The WinUI 3 process exited shortly after launch (code 0x{status:08X}).\n\nSee %LOCALAPPDATA%\\AndroidBackup\\Logs\\startup.log and launcher.log."
                ));
            }
            unsafe { CloseHandle(info.hProcess) };
        }
    }
}

#[cfg(windows)]
fn main() {
    windows_launcher::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This launcher supports Windows only.");
}
