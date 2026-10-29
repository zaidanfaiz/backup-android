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

fn copy_artifact(
    adb: &Adb,
    root: &Path,
    manifest: &mut Manifest,
    source: &str,
    target: &Path,
    kind: ArtifactKind,
    command: &str,
    cancel: &AtomicBool,
    notify: &mut impl FnMut(Progress),
) -> Result<()> {
    let display = relative_artifact_path(root, target)?;
    notify(Progress::Message(format!("Backing up {source}")));
    let mut next_report = 4 * 1024 * 1024;
    let (bytes, sha256) = adb.stream_to_file(command, target, cancel, |current| {
        if current >= next_report {
            notify(Progress::Transferred {
                file: display.clone(),
                bytes: current,
            });
            next_report = current + 4 * 1024 * 1024;
        }
    })?;
    manifest.artifacts.push(Artifact {
        kind,
        source: source.to_owned(),
        path: display,
        bytes,
        sha256,
    });
    manifest.save(root)?;
    notify(Progress::Message(format!(
        "Saved {source} ({:.1} MiB)",
        bytes as f64 / (1024.0 * 1024.0)
    )));
    Ok(())
}

fn copy_directory_if_present(
    adb: &Adb,
    backup_root: &Path,
    manifest: &mut Manifest,
    source: &str,
    target: &Path,
    kind: ArtifactKind,
    cancel: &AtomicBool,
    notify: &mut impl FnMut(Progress),
) -> Result<()> {
    ensure_not_cancelled(cancel)?;
    if !adb.directory_exists(source)? {
        notify(Progress::Message(format!("Not present, skipped: {source}")));
        return Ok(());
    }
    let command = remote_directory_command(source)?;
    copy_artifact(
        adb,
        backup_root,
        manifest,
        source,
        target,
        kind,
        &command,
        cancel,
        notify,
    )
}

fn backup_package(
    adb: &Adb,
    folder: &Path,
    manifest: &mut Manifest,
    package: &str,
    selected_storage: &[String],
    force_stop: bool,
    cancel: &AtomicBool,
    notify: &mut impl FnMut(Progress),
) -> Result<()> {
    if !valid_package(package) {
        bail!("Invalid selected package: {package}");
    }

    ensure_not_cancelled(cancel)?;
    if force_stop {
        adb.force_stop(package)?;
        notify(Progress::Message(format!("Force-stopped {package}")));
    }

    let apk_paths = adb.apk_paths(package)?;
    for (index, remote_path) in apk_paths.iter().enumerate() {
        ensure_not_cancelled(cancel)?;
        let original = remote_path.rsplit('/').next().context("Invalid APK path")?;
        let filename = format!("{index:02}-{}", sanitize_filename(original));
        let target = folder.join("apps").join(package).join("apk").join(filename);
        let command = format!("cat {}", shell_quote(remote_path));
        copy_artifact(
            adb,
            folder,
            manifest,
            remote_path,
            &target,
            ArtifactKind::Apk,
            &command,
            cancel,
            notify,
        )?;
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
        if category.is_some_and(|name| selected_storage.iter().any(|item| item == name)) {
            continue;
        }

        let source = format!("{parent}/{package}");
        let target = folder.join("apps").join(package).join(archive);
        copy_directory_if_present(
            adb,
            folder,
            manifest,
            &source,
            &target,
            ArtifactKind::AppData,
            cancel,
            notify,
        )?;
    }
    Ok(())
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
    let folder = options.destination.join(format!("android-{safe_serial}-{date}"));
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

    for package in &options.packages {
        backup_package(
            &adb,
            &folder,
            &mut manifest,
            package,
            &options.storage,
            options.force_stop,
            cancel,
            &mut notify,
        )
        .with_context(|| format!("Could not back up {package}"))?;
    }

    for category in &options.storage {
        ensure_not_cancelled(cancel)?;
        let source = format!("/storage/emulated/0/{category}");
        let archive = format!("{}.tar", category.replace('/', "_"));
        let target = folder.join("storage").join(archive);
        copy_directory_if_present(
            &adb,
            &folder,
            &mut manifest,
            &source,
            &target,
            ArtifactKind::SharedStorage,
            cancel,
            &mut notify,
        )?;
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
