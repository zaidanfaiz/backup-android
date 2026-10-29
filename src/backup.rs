use anyhow::{bail, Context, Result};
use chrono::Local;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    adb::{shell_quote, valid_package, Adb, Device},
    manifest::{relative_artifact_path, verify_backup, Artifact, ArtifactKind, Manifest},
};

pub const STORAGE_CATEGORIES: &[(&str, &str)] = &[
    ("DCIM", "Camera / DCIM"),
    ("Pictures", "Pictures"),
    ("Movies", "Videos / Movies"),
    ("Download", "Downloads"),
    ("Documents", "Documents"),
    ("Music", "Music"),
    ("Android/media", "Android media"),
    ("Android/data", "External app data"),
    ("Android/obb", "Game / OBB files"),
];

#[derive(Clone)]
pub struct BackupOptions {
    pub adb_path: PathBuf,
    pub device: Device,
    pub destination: PathBuf,
    pub packages: Vec<String>,
    pub storage: Vec<String>,
    pub force_stop: bool,
    pub backup_apk: bool,
    pub backup_private_data: bool,
    pub backup_external_data: bool,
    pub backup_obb: bool,
    pub backup_app_media: bool,
}

#[derive(Debug)]
pub enum Progress {
    Message(String),
    Transferred { file: String, bytes: u64 },
}

fn remote_directory_command(directory: &str) -> Result<String> {
    let (parent, name) = directory
        .rsplit_once('/')
        .context("Invalid remote directory")?;
    if name.is_empty() || parent.is_empty() {
        bail!("Invalid remote directory");
    }
    Ok(format!(
        "tar -cf - -C {} {}",
        shell_quote(parent),
        shell_quote(name)
    ))
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Backup cancelled");
    }
    Ok(())
}

/// Shared dependencies for one backup session. Keeping them together avoids
/// passing root paths, progress handlers and cancellation state through each step.
struct BackupSession<'a, F: FnMut(Progress)> {
    adb: &'a Adb,
    root: &'a Path,
    manifest: &'a mut Manifest,
    cancel: &'a AtomicBool,
    notify: &'a mut F,
}

impl<F: FnMut(Progress)> BackupSession<'_, F> {
    fn copy_artifact(
        &mut self,
        source: &str,
        target: &Path,
        kind: ArtifactKind,
        command: &str,
    ) -> Result<()> {
        let display = relative_artifact_path(self.root, target)?;
        (self.notify)(Progress::Message(format!("Backing up {source}")));
        let mut next_report = 4 * 1024 * 1024;
        let (bytes, sha256) = self
            .adb
            .stream_to_file(command, target, self.cancel, |current| {
                if current >= next_report {
                    (self.notify)(Progress::Transferred {
                        file: display.clone(),
                        bytes: current,
                    });
                    next_report = current + 4 * 1024 * 1024;
                }
            })?;
        self.manifest.artifacts.push(Artifact {
            kind,
            source: source.to_owned(),
            path: display,
            bytes,
            sha256,
        });
        self.manifest.save(self.root)?;
        (self.notify)(Progress::Message(format!(
            "Saved {source} ({:.1} MiB)",
            bytes as f64 / (1024.0 * 1024.0)
        )));
        Ok(())
    }

    fn copy_directory_if_present(
        &mut self,
        source: &str,
        target: &Path,
        kind: ArtifactKind,
    ) -> Result<()> {
        ensure_not_cancelled(self.cancel)?;
        if !self.adb.directory_exists(source)? {
            (self.notify)(Progress::Message(format!("Not present, skipped: {source}")));
            return Ok(());
        }
        let command = remote_directory_command(source)?;
        self.copy_artifact(source, target, kind, &command)
    }

    fn backup_package(
        &mut self,
        package: &str,
        selected_storage: &[String],
        options: &BackupOptions,
    ) -> Result<()> {
        if !valid_package(package) {
            bail!("Invalid selected package: {package}");
        }

        ensure_not_cancelled(self.cancel)?;
        if options.force_stop {
            self.adb.force_stop(package)?;
            (self.notify)(Progress::Message(format!("Force-stopped {package}")));
        }

        if options.backup_apk {
            let apk_paths = self.adb.apk_paths(package)?;
            for (index, remote_path) in apk_paths.iter().enumerate() {
                ensure_not_cancelled(self.cancel)?;
                let original = remote_path.rsplit('/').next().context("Invalid APK path")?;
                let filename = format!("{index:02}-{}", sanitize_filename(original));
                let target = self
                    .root
                    .join("apps")
                    .join(package)
                    .join("apk")
                    .join(filename);
                let command = format!("cat {}", shell_quote(remote_path));
                self.copy_artifact(remote_path, &target, ArtifactKind::Apk, &command)?;
            }
        }

        for (parent, archive, category) in [
            ("/data/user/0", "credential_encrypted.tar", None),
            ("/data/user_de/0", "device_encrypted.tar", None),
            (
                "/storage/emulated/0/Android/data",
                "external_data.tar",
                Some("Android/data"),
            ),
            (
                "/storage/emulated/0/Android/obb",
                "obb.tar",
                Some("Android/obb"),
            ),
            (
                "/storage/emulated/0/Android/media",
                "media.tar",
                Some("Android/media"),
            ),
        ] {
            let component_enabled = match archive {
                "credential_encrypted.tar" | "device_encrypted.tar" => options.backup_private_data,
                "external_data.tar" => options.backup_external_data,
                "obb.tar" => options.backup_obb,
                "media.tar" => options.backup_app_media,
                _ => false,
            };
            if !component_enabled
                || category.is_some_and(|name| selected_storage.iter().any(|item| item == name))
            {
                continue;
            }

            let source = format!("{parent}/{package}");
            let target = self.root.join("apps").join(package).join(archive);
            self.copy_directory_if_present(&source, &target, ArtifactKind::AppData)?;
        }
        Ok(())
    }
}

/// A successful return means all selected streams have passed SHA-256 validation.
/// On failure, a manifest marked complete=false remains alongside any finished artifacts.
pub fn run_backup(
    options: BackupOptions,
    cancel: &AtomicBool,
    mut notify: impl FnMut(Progress),
) -> Result<PathBuf> {
    if options.packages.is_empty() && options.storage.is_empty() {
        bail!("Select at least one app or storage category");
    }
    let app_component_selected = options.backup_apk
        || options.backup_private_data
        || options.backup_external_data
        || options.backup_obb
        || options.backup_app_media;
    if !options.packages.is_empty() && !app_component_selected && options.storage.is_empty() {
        bail!("Select at least one app backup component");
    }
    for category in &options.storage {
        if !STORAGE_CATEGORIES.iter().any(|(path, _)| path == category) {
            bail!("Unknown storage category: {category}");
        }
    }
    let adb = Adb::new(options.adb_path, &options.device)?;
    adb.check_ready()?;

    fs::create_dir_all(&options.destination)?;
    let safe_serial = sanitize_filename(&options.device.serial);
    let date = Local::now().format("%Y%m%d-%H%M%S-%3f");
    let folder = options
        .destination
        .join(format!("android-{safe_serial}-{date}"));
    fs::create_dir(&folder).context("Unable to create an empty backup directory")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
    }

    let mut manifest = Manifest::new(&options.device, options.packages.clone())?;
    manifest.save(&folder)?;
    notify(Progress::Message(format!(
        "Connected: {} (Android {})",
        options.device.model, options.device.android_version
    )));

    {
        let mut session = BackupSession {
            adb: &adb,
            root: &folder,
            manifest: &mut manifest,
            cancel,
            notify: &mut notify,
        };

        for package in &options.packages {
            session
                .backup_package(package, &options.storage, &options)
                .with_context(|| format!("Could not back up {package}"))?;
        }

        for category in &options.storage {
            ensure_not_cancelled(cancel)?;
            let source = format!("/storage/emulated/0/{category}");
            let archive = format!("{}.tar", category.replace('/', "_"));
            let target = folder.join("storage").join(archive);
            session.copy_directory_if_present(&source, &target, ArtifactKind::SharedStorage)?;
        }
    }

    if manifest.artifacts.is_empty() {
        bail!("No backup artifacts were produced");
    }

    ensure_not_cancelled(cancel)?;
    manifest.complete = true;
    manifest.save(&folder)?;
    match verify_backup(&folder) {
        Ok(count) => {
            notify(Progress::Message(format!(
                "Verified {count} artifacts against SHA-256"
            )));
            Ok(folder)
        }
        Err(error) => {
            manifest.complete = false;
            manifest.save(&folder)?;
            Err(error.context("Backup validation failed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths_are_quoted() {
        assert_eq!(
            remote_directory_command("/data/user/0/com.example.app").unwrap(),
            "tar -cf - -C '/data/user/0' 'com.example.app'"
        );
    }

    #[test]
    fn filenames_are_sanitized() {
        assert_eq!(sanitize_filename("hello/world.apk"), "hello_world.apk");
    }
}
