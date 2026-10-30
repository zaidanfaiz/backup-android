use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
};

use crate::adb::Device;

pub const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Apk,
    AppData,
    SharedStorage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub source: String,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Manifest {
    pub format_version: u32,
    pub created_at_utc: String,
    pub serial: String,
    pub model: String,
    pub android_version: String,
    pub root_method: String,
    pub user_id: u32,
    pub selected_packages: Vec<String>,
    pub artifacts: Vec<Artifact>,
    pub warnings: Vec<String>,
    pub complete: bool,
}

impl Manifest {
    pub fn new(device: &Device, packages: Vec<String>) -> Result<Self> {
        let root = device.root.context("Root is required")?;
        Ok(Self {
            format_version: FORMAT_VERSION,
            created_at_utc: Utc::now().to_rfc3339(),
            serial: device.serial.clone(),
            model: device.model.clone(),
            android_version: device.android_version.clone(),
            root_method: root.label().to_owned(),
            user_id: 0,
            selected_packages: packages,
            artifacts: Vec::new(),
            warnings: Vec::new(),
            complete: false,
        })
    }

    pub fn save(&self, directory: &Path) -> Result<()> {
        let target = directory.join("manifest.json");
        let temporary = directory.join("manifest.json.tmp");
        let json = serde_json::to_vec_pretty(self)?;
        fs::write(&temporary, json).context("Unable to write backup manifest")?;
        fs::rename(&temporary, &target).context("Unable to commit backup manifest")
    }
}

pub fn relative_artifact_path(folder: &Path, file: &Path) -> Result<String> {
    let path = file.strip_prefix(folder)?;
    if !is_safe_relative(path) {
        bail!("Artifact path must remain inside the backup directory");
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

fn is_safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn digest_file(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        total += count as u64;
    }
    Ok((total, format!("{:x}", hash.finalize())))
}

pub fn verify_backup(directory: &Path) -> Result<usize> {
    let bytes =
        fs::read(directory.join("manifest.json")).context("Unable to read manifest.json")?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.format_version != FORMAT_VERSION {
        bail!("Unsupported backup format version");
    }
    if !manifest.complete {
        bail!("Backup was interrupted: manifest is marked incomplete");
    }
    if manifest.artifacts.is_empty() {
        bail!("No files to verify");
    }

    for artifact in &manifest.artifacts {
        let relative = PathBuf::from(&artifact.path);
        if !is_safe_relative(&relative) {
            bail!("Manifest contains an unsafe relative path");
        }
        let target = directory.join(&relative);
        let (actual_size, actual_sha) =
            digest_file(&target).with_context(|| format!("Unable to read {}", artifact.path))?;
        if actual_size != artifact.bytes || actual_sha != artifact.sha256 {
            bail!("Backup verification failed: {}", artifact.path);
        }
    }
    Ok(manifest.artifacts.len())
}


/// Verify only artifacts belonging to one selected app, avoiding a full
/// multi-app backup rehash each time an app is checked or restored.
/// Callers must separately validate manifest completeness and device identity.
pub fn verify_package_artifacts(directory: &Path, package: &str) -> Result<usize> {
    if !crate::adb::valid_package(package) {
        bail!("Invalid Android package name");
    }
    let bytes = fs::read(directory.join("manifest.json")).context("Unable to read manifest.json")?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.format_version != FORMAT_VERSION || !manifest.complete {
        bail!("Incomplete backup or unsupported format");
    }
    if !manifest.selected_packages.iter().any(|item| item == package) {
        bail!("Package is not included in this backup");
    }
    let prefix = format!("apps/{package}/");
    let mut verified = 0usize;
    for artifact in &manifest.artifacts {
        let relative = PathBuf::from(&artifact.path);
        if !is_safe_relative(&relative) {
            bail!("Manifest contains an unsafe relative path");
        }
        if !artifact.path.replace('\\', "/").starts_with(&prefix) {
            continue;
        }
        let target = directory.join(&relative);
        let (size, sha) = digest_file(&target)
            .with_context(|| format!("Unable to read {}", artifact.path))?;
        if size != artifact.bytes || sha != artifact.sha256 {
            bail!("Backup verification failed: {}", artifact.path);
        }
        verified += 1;
    }
    if verified == 0 {
        bail!("No backed-up artifacts found for {package}");
    }
    Ok(verified)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_rejects_invalid_package_before_reading_files() {
        let unused = std::env::temp_dir();
        assert!(verify_package_artifacts(&unused, "../traversal").is_err());
    }

    #[test]
    fn reject_unsafe_manifest_paths() {
        assert!(is_safe_relative(Path::new("apps/demo/base.apk")));
        assert!(!is_safe_relative(Path::new("../secret")));
        assert!(!is_safe_relative(Path::new("/absolute/file")));
        assert!(!is_safe_relative(Path::new(".")));
    }
}
