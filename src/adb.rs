use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootAccess {
    Adbd,
    Su,
}

impl RootAccess {
    pub fn label(self) -> &'static str {
        match self {
            Self::Adbd => "adbd (uid 0)",
            Self::Su => "su (uid 0)",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Device {
    pub serial: String,
    pub model: String,
    pub android_version: String,
    pub root: Option<RootAccess>,
}

#[derive(Clone)]
pub struct Adb {
    binary: PathBuf,
    serial: String,
    root: RootAccess,
}

fn adb_command(binary: &Path, serial: Option<&str>) -> Command {
    let mut cmd = Command::new(binary);
    if let Some(serial) = serial {
        cmd.arg("-s").arg(serial);
    }
    cmd
}

fn successful_text(mut cmd: Command) -> Result<String> {
    let output = cmd
        .output()
        .context("Failed to execute adb. Check the adb path.")?;
    if !output.status.success() {
        bail!(
            "adb exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn shell(binary: &Path, serial: &str, script: &str) -> Result<String> {
    let mut cmd = adb_command(binary, Some(serial));
    cmd.arg("shell").arg(script);
    successful_text(cmd)
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn valid_package(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with('.')
        && name.contains('.')
        && name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'.')
}

pub fn list_devices(binary: &Path) -> Result<Vec<String>> {
    let mut cmd = adb_command(binary, None);
    cmd.arg("devices");
    let output = successful_text(cmd)?;
    Ok(output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let state = fields.next()?;
            (state == "device").then(|| serial.to_owned())
        })
        .collect())
}

pub fn inspect(binary: &Path, serial: &str) -> Result<Device> {
    let model = shell(binary, serial, "getprop ro.product.model")?;
    let android_version = shell(binary, serial, "getprop ro.build.version.release")?;

    let root = if shell(binary, serial, "id -u")
        .map(|output| output.trim() == "0")
        .unwrap_or(false)
    {
        Some(RootAccess::Adbd)
    } else if shell(binary, serial, "su -c 'id -u'")
        .map(|output| output.trim() == "0")
        .unwrap_or(false)
    {
        Some(RootAccess::Su)
    } else {
        None
    };

    Ok(Device {
        serial: serial.to_owned(),
        model,
        android_version,
        root,
    })
}

impl Adb {
    pub fn new(binary: PathBuf, device: &Device) -> Result<Self> {
        let root = device.root.context("A working root shell is required")?;
        Ok(Self {
            binary,
            serial: device.serial.clone(),
            root,
        })
    }

    fn root_script(&self, script: &str) -> String {
        match self.root {
            RootAccess::Adbd => script.to_owned(),
            RootAccess::Su => format!("su -c {}", shell_quote(script)),
        }
    }

    pub fn root_text(&self, script: &str) -> Result<String> {
        shell(&self.binary, &self.serial, &self.root_script(script))
    }

    pub fn check_ready(&self) -> Result<()> {
        if self.root_text("id -u")?.trim() != "0" {
            bail!("Root permission has expired or been revoked");
        }
        if self.root_text("command -v tar")?.is_empty() {
            bail!("The Android device does not provide tar");
        }
        Ok(())
    }

    pub fn directory_exists(&self, directory: &str) -> Result<bool> {
        let script = format!(
            "if test -d {}; then echo PRESENT; else echo ABSENT; fi",
            shell_quote(directory)
        );
        match self.root_text(&script)?.trim() {
            "PRESENT" => Ok(true),
            "ABSENT" => Ok(false),
            other => bail!("Unexpected directory check response: {other}"),
        }
    }

    pub fn packages(&self) -> Result<Vec<String>> {
        let output = self.root_text("pm list packages -3 --user 0")?;
        let mut packages: Vec<String> = output
            .lines()
            .filter_map(|line| line.trim().strip_prefix("package:"))
            .filter(|name| valid_package(name))
            .map(str::to_owned)
            .collect();
        packages.sort_unstable();
        packages.dedup();
        Ok(packages)
    }

    pub fn apk_paths(&self, package: &str) -> Result<Vec<String>> {
        if !valid_package(package) {
            bail!("Invalid Android package name");
        }
        let output = self.root_text(&format!("pm path {}", shell_quote(package)))?;
        let paths: Vec<_> = output
            .lines()
            .filter_map(|line| line.trim().strip_prefix("package:"))
            .filter(|path| path.starts_with('/') && path.ends_with(".apk"))
            .map(str::to_owned)
            .collect();
        if paths.is_empty() {
            bail!("No APK paths returned for {package}");
        }
        Ok(paths)
    }

    pub fn force_stop(&self, package: &str) -> Result<()> {
        if !valid_package(package) {
            bail!("Invalid Android package name");
        }
        self.root_text(&format!("am force-stop {}", shell_quote(package)))?;
        Ok(())
    }

    /// Streams binary stdout from adb exec-out into a temporary file.
    /// The destination is published only after a successful exit and fsync.
    pub fn stream_to_file(
        &self,
        script: &str,
        destination: &Path,
        cancel: &AtomicBool,
        mut on_bytes: impl FnMut(u64),
    ) -> Result<(u64, String)> {
        if cancel.load(Ordering::Relaxed) {
            bail!("Backup cancelled");
        }
        let name = destination
            .file_name()
            .context("Artifact has no filename")?
            .to_string_lossy();
        let partial = destination.with_file_name(format!("{name}.part"));
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut cmd = adb_command(&self.binary, Some(&self.serial));
        cmd.arg("exec-out")
            .arg(self.root_script(script))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Unable to launch adb exec-out")?;
        let mut stdout = child.stdout.take().context("Missing adb stdout")?;
        let mut stderr = child.stderr.take().context("Missing adb stderr")?;
        let stderr_reader = thread::spawn(move || {
            let mut output = Vec::new();
            let _ = stderr.read_to_end(&mut output);
            String::from_utf8_lossy(&output).into_owned()
        });

        let copied = (|| -> Result<(u64, String)> {
            let mut file = File::create(&partial)?;
            let mut digest = Sha256::new();
            let mut total = 0_u64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    bail!("Backup cancelled");
                }
                let count = stdout.read(&mut buffer).context("ADB stream interrupted")?;
                if count == 0 {
                    break;
                }
                file.write_all(&buffer[..count])?;
                digest.update(&buffer[..count]);
                total += count as u64;
                on_bytes(total);
            }
            file.sync_all()?;
            Ok((total, format!("{:x}", digest.finalize())))
        })();

        if copied.is_err() {
            let _ = child.kill();
        }
        drop(stdout);
        let status = child.wait().context("Unable to wait for adb")?;
        let stderr = stderr_reader.join().unwrap_or_default();

        let outcome = match copied {
            Ok((size, digest)) if status.success() && size > 0 => {
                fs::rename(&partial, destination)?;
                Ok((size, digest))
            }
            Ok(_) => Err(anyhow!(
                "Device stream failed (status {status}): {}",
                stderr.trim()
            )),
            Err(error) => Err(error.context(format!("ADB stderr: {}", stderr.trim()))),
        };
        if outcome.is_err() {
            let _ = fs::remove_file(partial);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_are_safely_restricted() {
        assert!(valid_package("com.example.app"));
        assert!(valid_package("org.example_app.release"));
        assert!(!valid_package("com.example;rm -rf /"));
        assert!(!valid_package("../data"));
        assert!(!valid_package(""));
    }

    #[test]
    fn quotes_shell_literals() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
