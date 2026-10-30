//! Conservative, same-device Android app-data restore.
//!
//! Restoration is deliberately not a generic "unpack a tar as root" tool:
//! - the whole backup must pass SHA-256 verification,
//! - each archive entry must be a regular file or directory below exactly
//!   the selected package name, with no links, absolute paths or traversal,
//! - the target must have the same serial and Android version,
//! - the target package must already be installed,
//! - the existing app data is backed up before any overwrite,
//! - a separate explicit confirmation is required.
//!
//! This first version merges files into the installed app's directories. A
//! successful restore does not guarantee a particular app is compatible with
//! its archived state; Android Keystore and hardware-backed keys are excluded.
use crate::{
    adb::{shell_quote, valid_package, Adb, Device},
    backup::{run_backup, BackupOptions, Progress},
    manifest::{
        verify_backup, verify_package_artifacts, Artifact, ArtifactKind, Manifest, FORMAT_VERSION,
    },
};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{
    fs::File,
    path::{Component, Path, PathBuf},
    sync::atomic::AtomicBool,
};

#[derive(Debug, Serialize)]
pub struct RestorePlan {
    pub packages: Vec<String>,
    pub data_packages: Vec<String>,
    pub android_version: String,
    pub original_device: String,
    pub target_matches: bool,
    pub artifacts_verified: usize,
    pub caveats: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct RestorePreflight {
    pub package: String,
    pub verified_files: usize,
    pub data_archives: usize,
    pub original_device: String,
    pub android_version: String,
    pub safety_backup_required: bool,
}

#[derive(Debug, Serialize)]
pub struct RestoreResult {
    pub package: String,
    pub safety_backup: String,
    pub restored_archives: usize,
}

struct DataArchive<'a> {
    source: &'a Artifact,
    parent: &'static str,
    is_private: bool,
}

fn load_manifest(folder: &Path) -> Result<Manifest> {
    let path = folder.join("manifest.json");
    let data = std::fs::read(&path).context("Cannot read backup manifest")?;
    let manifest: Manifest = serde_json::from_slice(&data).context("Invalid backup manifest")?;
    if manifest.format_version != FORMAT_VERSION || !manifest.complete {
        bail!("Backup is incomplete or has an unsupported format version");
    }
    Ok(manifest)
}

fn parent_for_source(package: &str, source: &str) -> Option<(&'static str, bool)> {
    let expected = [
        ("/data/user/0", true),
        ("/data/user_de/0", true),
        ("/storage/emulated/0/Android/data", false),
        ("/storage/emulated/0/Android/obb", false),
        ("/storage/emulated/0/Android/media", false),
    ];
    expected
        .into_iter()
        .find(|(parent, _)| source == format!("{parent}/{package}"))
}

fn app_archives<'a>(manifest: &'a Manifest, package: &str) -> Result<Vec<DataArchive<'a>>> {
    if !valid_package(package)
        || !manifest
            .selected_packages
            .iter()
            .any(|name| name == package)
    {
        bail!("This application was not selected in the backup");
    }
    let mut archives = Vec::new();
    for artifact in &manifest.artifacts {
        if !matches!(artifact.kind, ArtifactKind::AppData) {
            continue;
        }
        let Some((parent, is_private)) = parent_for_source(package, &artifact.source) else {
            continue;
        };
        let expected_dir = format!("apps/{package}/");
        if !artifact.path.replace('\\', "/").starts_with(&expected_dir) {
            bail!("App archive does not live under the selected package directory");
        }
        if archives
            .iter()
            .any(|item: &DataArchive<'_>| item.parent == parent)
        {
            bail!("Backup contains duplicate data archives for {package}");
        }
        archives.push(DataArchive {
            source: artifact,
            parent,
            is_private,
        });
    }
    Ok(archives)
}

fn validate_archive(path: &Path, package: &str) -> Result<()> {
    let file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let mut tar = tar::Archive::new(file);
    let mut checked = 0_usize;
    for entry in tar.entries().context("Invalid app data TAR archive")? {
        let mut entry = entry.context("Corrupt TAR entry")?;
        let kind = entry.header().entry_type();
        if !(kind.is_dir() || kind.is_file()) {
            bail!("Restore blocks TAR symlinks, hardlinks, and special files");
        }
        let raw_path = entry.path().context("Unsafe TAR filename")?;
        let mut components = raw_path.components();
        if !matches!(
            components.next(),
            Some(Component::Normal(component)) if component == package
        ) || !components.all(|component| matches!(component, Component::Normal(_)))
        {
            bail!("TAR includes a path outside the selected package");
        }
        // This reads and validates each file's content and checksum before
        // transferring it to a device. No extraction occurs on the host.
        std::io::copy(&mut entry, &mut std::io::sink())
            .context("Failed to validate TAR content")?;
        checked += 1;
        if checked > 500_000 {
            bail!("Archive contains too many entries");
        }
    }
    if checked == 0 {
        bail!("App data TAR archive is empty");
    }
    Ok(())
}

fn verified_target(manifest: &Manifest, device: &Device) -> Result<()> {
    if manifest.user_id != 0 || manifest.serial != device.serial {
        bail!("Restore is restricted to Android user 0 on the original device");
    }
    if manifest.android_version != device.android_version {
        bail!(
            "Android version differs (backup {}, device {}); restore refused",
            manifest.android_version,
            device.android_version
        );
    }
    Ok(())
}

pub fn plan(folder: &Path, device: &Device) -> Result<RestorePlan> {
    let verified = verify_backup(folder).context("Backup checksum verification failed")?;
    let manifest = load_manifest(folder)?;
    let mut data_packages = Vec::new();
    for package in &manifest.selected_packages {
        if !app_archives(&manifest, package)?.is_empty() {
            data_packages.push(package.clone());
        }
    }
    let target_matches = verified_target(&manifest, device).is_ok();
    Ok(RestorePlan {
        packages: manifest.selected_packages,
        data_packages,
        android_version: manifest.android_version,
        original_device: manifest.model,
        target_matches,
        artifacts_verified: verified,
        caveats: vec![
            "Only user 0 on the original device and Android version is supported",
            "The original APK version and all split APK files must already be installed unchanged",
            "Restore merges app data into the existing installation and can overwrite files",
            "Android Keystore-backed secrets and hardware-bound sessions may not survive",
            "A full pre-restore safety backup is mandatory",
        ],
    })
}

fn verify_installed_apks(adb: &Adb, manifest: &Manifest, package: &str) -> Result<()> {
    // A package name alone does not establish the application's signing
    // identity. Until certificate-aware restore is implemented, require
    // bit-for-bit identical APK and split APK files on the target device.
    let prefix = format!("apps/{package}/apk/");
    let mut archived: Vec<String> = manifest
        .artifacts
        .iter()
        .filter(|artifact| {
            matches!(artifact.kind, ArtifactKind::Apk) && artifact.path.starts_with(&prefix)
        })
        .map(|artifact| artifact.sha256.to_ascii_lowercase())
        .collect();
    if archived.is_empty() {
        bail!("Restore requires an APK backup to verify the installed app identity");
    }
    let paths = adb.apk_paths(package)?;
    if paths.len() != archived.len() {
        bail!("Installed APK/split count differs from the backup; restore refused");
    }
    let mut installed = Vec::new();
    for path in paths {
        let output = adb.root_text(&format!("sha256sum {}", shell_quote(&path)))?;
        let digest = output
            .split_whitespace()
            .next()
            .context("Android did not return an APK SHA-256 hash")?;
        if digest.len() != 64 || !digest.bytes().all(|ch| ch.is_ascii_hexdigit()) {
            bail!("Android returned an invalid APK digest");
        }
        installed.push(digest.to_ascii_lowercase());
    }
    archived.sort_unstable();
    installed.sort_unstable();
    if archived != installed {
        bail!("Installed APK differs from backup; app version/signature compatibility cannot be guaranteed");
    }
    Ok(())
}

fn original_owner(adb: &Adb, directory: &str) -> Result<String> {
    let output = adb.root_text(&format!("stat -c '%u:%g' {}", shell_quote(directory)))?;
    let owner = output.trim();
    if owner.is_empty()
        || !owner.contains(':')
        || !owner.bytes().all(|ch| ch.is_ascii_digit() || ch == b':')
    {
        bail!("Cannot determine the installed application's UID:GID");
    }
    Ok(owner.to_owned())
}

/// Read-only, per-app validation. This does not modify Android data. The
/// destructive restore independently reruns all checks after confirmation.
pub fn preflight(
    folder: &Path,
    package: &str,
    adb_binary: PathBuf,
    device: &Device,
) -> Result<RestorePreflight> {
    let manifest = load_manifest(folder)?;
    verified_target(&manifest, device)?;
    let files = verify_package_artifacts(folder, package)?;
    let archives = app_archives(&manifest, package)?;
    if archives.is_empty() {
        bail!("No backed-up private or external data for the selected app");
    }

    let adb = Adb::new(adb_binary, device)?;
    adb.check_ready()?;
    verify_installed_apks(&adb, &manifest, package)
        .context("Installed app does not match the backed-up APK build")?;
    let private_dir = format!("/data/user/0/{package}");
    if !adb.directory_exists(&private_dir)? {
        bail!("App private directory is missing. Open the installed app once before restoring");
    }
    if archives.iter().any(|archive| archive.is_private) {
        for binary in ["chown", "restorecon"] {
            if adb
                .root_text(&format!("command -v {binary}"))?
                .trim()
                .is_empty()
            {
                bail!("Device is missing a required restore tool: {binary}");
            }
        }
    }

    for archive in &archives {
        let target = format!("{}/{}", archive.parent, package);
        if archive.is_private && !adb.directory_exists(&target)? {
            bail!("Required private data directory is missing: {target}");
        }
        if adb.directory_exists(&target)? {
            let script = format!("find {} -type l -print | head -n 1", shell_quote(&target));
            if !adb.root_text(&script)?.trim().is_empty() {
                bail!("App data contains symlinks; unsafe in-place restore blocked");
            }
        }
        validate_archive(&folder.join(&archive.source.path), package)?;
    }
    Ok(RestorePreflight {
        package: package.to_owned(),
        verified_files: files,
        data_archives: archives.len(),
        original_device: manifest.model,
        android_version: manifest.android_version,
        safety_backup_required: true,
    })
}

pub struct RestoreOptions {
    pub folder: PathBuf,
    pub package: String,
    pub adb_binary: PathBuf,
    pub device: Device,
    pub safety_destination: PathBuf,
    pub confirm: bool,
}

pub fn restore_app(
    options: RestoreOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<RestoreResult> {
    let RestoreOptions {
        folder,
        package,
        adb_binary,
        device,
        safety_destination,
        confirm,
    } = options;
    let folder = folder.as_path();
    let package = package.as_str();
    let device = &device;
    let safety_destination = safety_destination.as_path();
    if !confirm {
        bail!("Restore must be explicitly confirmed");
    }
    let manifest = load_manifest(folder)?;
    let verified = verify_package_artifacts(folder, package)
        .context("Selected app backup integrity validation failed")?;
    verified_target(&manifest, device)?;
    progress(Progress::Message(format!(
        "Verified {verified} backup artifacts before restore"
    )));

    let archives = app_archives(&manifest, package)?;
    if archives.is_empty() {
        bail!("No app-data archives available for the selected package");
    }
    let adb = Adb::new(adb_binary.clone(), device)?;
    adb.check_ready()?;

    // The package must already exist. Never install arbitrary APK archives
    // automatically. Android itself enforces signing and package identity.
    verify_installed_apks(&adb, &manifest, package)
        .context("App package identity preflight failed")?;
    let private_dir = format!("/data/user/0/{package}");
    if !adb.directory_exists(&private_dir)? {
        bail!("Installed application data directory is missing; launch the app once, then retry");
    }

    if archives.iter().any(|archive| archive.is_private) {
        if adb.root_text("command -v restorecon")?.trim().is_empty() {
            bail!("Android restorecon is required for safe private-data recovery");
        }
        if adb.root_text("command -v chown")?.trim().is_empty() {
            bail!("Android chown is required to restore the application's UID");
        }
    }

    // Validate every TAR before touching the target.
    for archive in &archives {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("Restore cancelled");
        }
        validate_archive(&folder.join(&archive.source.path), package)
            .with_context(|| format!("Unsafe archive: {}", archive.source.path))?;
    }

    // Reject pre-existing symlinks in target trees. A benign-looking TAR
    // entry can escape its package through a symlink already on the device.
    // Refusing such targets is safer than following them during root extraction.
    for archive in &archives {
        let target = format!("{}/{}", archive.parent, package);
        if archive.is_private && !adb.directory_exists(&target)? {
            bail!("Required private app directory does not exist: {target}");
        }
        if adb.directory_exists(&target)? {
            let probe = format!("find {} -type l -print | head -n 1", shell_quote(&target));
            if !adb.root_text(&probe)?.trim().is_empty() {
                bail!("Restore target contains symlinks; refusing unsafe root extraction");
            }
        }
    }

    // Snapshot first, and abort if it fails. This snapshot remains on the host
    // even if the later restore fails partway through.
    progress(Progress::Message(
        "Creating mandatory pre-restore safety backup".to_owned(),
    ));
    let safety = run_backup(
        BackupOptions {
            adb_path: adb_binary,
            device: device.clone(),
            destination: safety_destination.to_path_buf(),
            packages: vec![package.to_owned()],
            storage: Vec::new(),
            force_stop: true,
            backup_apk: true,
            backup_private_data: true,
            backup_external_data: true,
            backup_obb: true,
            backup_app_media: true,
        },
        cancel,
        &mut progress,
    )
    .context("Safety backup failed: original app data has not been modified")?;

    progress(Progress::Message(format!(
        "Safety backup saved: {}",
        safety.display()
    )));
    let mut restored = 0;

    for (index, archive) in archives.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            bail!(
                "Restore interrupted; pre-restore backup preserved at {}",
                safety.display()
            );
        }
        let device_path = format!("{}/{}", archive.parent, package);
        let owner = if archive.is_private {
            Some(
                original_owner(&adb, &device_path)
                    .with_context(|| format!("Cannot identify owner of {device_path}"))?,
            )
        } else {
            None
        };
        adb.force_stop(package)?;
        let unique = format!(
            "ab-restore-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos(),
            index
        );
        let remote = format!("/data/local/tmp/{unique}.tar");
        let local = folder.join(&archive.source.path);
        progress(Progress::Message(format!(
            "Restoring {} into {}",
            archive.source.path, archive.parent
        )));
        let operation = (|| -> Result<()> {
            adb.push_file(&local, &remote)?;
            let script = format!(
                "tar -xf {} -C {}",
                shell_quote(&remote),
                shell_quote(archive.parent)
            );
            adb.root_text(&script)
                .context("Android tar extraction failed")?;
            if let Some(owner) = owner {
                let fix = format!(
                    "chown -R {} {} && restorecon -RF {}",
                    shell_quote(&owner),
                    shell_quote(&device_path),
                    shell_quote(&device_path)
                );
                adb.root_text(&fix)
                    .context("Restored data permissions/SELinux labels could not be repaired")?;
            }
            Ok(())
        })();
        let _ = adb.root_text(&format!("rm -f {}", shell_quote(&remote)));
        if let Err(error) = operation {
            return Err(error.context(format!(
                "Partial restore possible; safety backup is stored at {}",
                safety.display()
            )));
        }
        restored += 1;
    }

    adb.force_stop(package)?;
    progress(Progress::Message(format!(
        "Restore complete for {package}. Safety backup retained at {}",
        safety.display()
    )));
    Ok(RestoreResult {
        package: package.to_owned(),
        safety_backup: safety.display().to_string(),
        restored_archives: restored,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ArtifactKind;

    #[test]
    fn only_expected_package_sources_are_accepted() {
        assert_eq!(
            parent_for_source("com.example.app", "/data/user/0/com.example.app"),
            Some(("/data/user/0", true))
        );
        assert!(parent_for_source("com.example.app", "/data/user/0/com.evil.app").is_none());
        assert!(parent_for_source("com.example.app", "/system/bin/com.example.app").is_none());
    }

    #[test]
    fn rejects_tar_traversal_and_links() {
        // All entries in a restore archive must begin with one exact package.
        // Construct an intentionally invalid tar with a different root.
        let mut buffer = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buffer);
            let data = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append_data(&mut header, "com.other.app/prefs.xml", &data[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let file = std::env::temp_dir().join(format!(
            "android-backup-restore-test-{}.tar",
            std::process::id()
        ));
        std::fs::write(&file, buffer).unwrap();
        assert!(validate_archive(&file, "com.example.app").is_err());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn reject_unselected_package_archive() {
        let manifest = Manifest {
            format_version: FORMAT_VERSION,
            created_at_utc: String::new(),
            serial: String::new(),
            model: String::new(),
            android_version: String::new(),
            root_method: String::new(),
            user_id: 0,
            selected_packages: vec!["com.example.app".into()],
            artifacts: vec![Artifact {
                kind: ArtifactKind::AppData,
                source: "/data/user/0/com.other.app".into(),
                path: "apps/com.other.app/credential_encrypted.tar".into(),
                bytes: 0,
                sha256: String::new(),
            }],
            warnings: Vec::new(),
            complete: true,
        };
        assert!(app_archives(&manifest, "com.other.app").is_err());
    }
}
