//! JSON Lines protocol between the native WinUI 3 frontend and the Rust backup engine.
//! One UTF-8 JSON request is supplied via stdin; responses are newline-delimited JSON.
//! A failed operation emits an error event and returns a nonzero exit code.
mod adb;
mod backup;
mod manifest;
mod ransomware;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::atomic::AtomicBool,
};

#[derive(Deserialize)]
struct Request {
    action: String,
    #[serde(default = "default_adb")]
    adb_path: PathBuf,
    serial: Option<String>,
    destination: Option<PathBuf>,
    folder: Option<PathBuf>,
    #[serde(default)]
    packages: Vec<String>,
    #[serde(default)]
    storage: Vec<String>,
    #[serde(default)]
    force_stop: bool,
    #[serde(default = "enabled_by_default")]
    backup_apk: bool,
    #[serde(default = "enabled_by_default")]
    backup_private_data: bool,
    #[serde(default = "enabled_by_default")]
    backup_external_data: bool,
    #[serde(default = "enabled_by_default")]
    backup_obb: bool,
    #[serde(default = "enabled_by_default")]
    backup_app_media: bool,
}

fn enabled_by_default() -> bool {
    true
}

fn default_adb() -> PathBuf {
    PathBuf::from("adb")
}

fn emit(event: Value) {
    println!("{event}");
    let _ = io::stdout().flush();
}

fn required<T>(value: Option<T>, name: &str) -> Result<T> {
    value.with_context(|| format!("Missing required request field: {name}"))
}

fn execute(request: Request) -> Result<()> {
    match request.action.as_str() {
        "devices" => {
            let devices = adb::list_devices(&request.adb_path)?;
            emit(json!({ "type": "devices", "devices": devices }));
        }
        "inspect" => {
            let serial = required(request.serial, "serial")?;
            let device = adb::inspect(&request.adb_path, &serial)?;
            let packages = if device.root.is_some() {
                adb::Adb::new(request.adb_path, &device)?.packages()?
            } else {
                vec![]
            };
            emit(json!({
                "type": "device",
                "serial": device.serial,
                "model": device.model,
                "android_version": device.android_version,
                "root": device.root.map(|value| value.label()),
                "packages": packages
            }));
        }
        "backup" => {
            let serial = required(request.serial, "serial")?;
            let destination = required(request.destination, "destination")?;
            let device = adb::inspect(&request.adb_path, &serial)?;
            if device.root.is_none() {
                bail!("Root shell is unavailable. Activate temp root and inspect again.");
            }
            let cancel = AtomicBool::new(false);
            let options = backup::BackupOptions {
                adb_path: request.adb_path,
                device,
                destination,
                packages: request.packages,
                storage: request.storage,
                force_stop: request.force_stop,
                backup_apk: request.backup_apk,
                backup_private_data: request.backup_private_data,
                backup_external_data: request.backup_external_data,
                backup_obb: request.backup_obb,
                backup_app_media: request.backup_app_media,
            };
            let path = backup::run_backup(options, &cancel, |progress| match progress {
                backup::Progress::Message(message) => {
                    emit(json!({ "type": "progress", "message": message }));
                }
                backup::Progress::Transferred { file, bytes } => {
                    emit(json!({ "type": "transferred", "file": file, "bytes": bytes }));
                }
            })?;
            emit(json!({ "type": "complete", "folder": path }));
        }
        "verify" => {
            let folder = required(request.folder, "folder")?;
            let count = manifest::verify_backup(&folder)?;
            emit(json!({ "type": "verified", "count": count }));
        }
        "scan_ransomware" => {
            let serial = required(request.serial, "serial")?;
            let device = adb::inspect(&request.adb_path, &serial)?;
            if device.root.is_none() {
                bail!("Ransomware scan requires an active temp root shell.");
            }
            let adb = adb::Adb::new(request.adb_path, &device)?;
            let report = ransomware::scan_device(&adb, |message| {
                emit(json!({ "type": "progress", "message": message }));
            })?;
            emit(json!({ "type": "ransomware_scan", "report": report }));
        }
        _ => bail!(
            "Unsupported action. Valid actions: devices, inspect, backup, verify, scan_ransomware"
        ),
    }
    Ok(())
}

fn main() {
    if let Err(error) = (|| -> Result<()> {
        let stdin = io::stdin();
        let mut input = String::new();
        stdin.lock().read_line(&mut input)?;
        if input.trim().is_empty() {
            bail!("No JSON request supplied");
        }
        let request: Request = serde_json::from_str(&input).context("Invalid JSON request")?;
        execute(request)
    })() {
        emit(json!({ "type": "error", "message": format!("{error:#}") }));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_backup_request() {
        let request: Request = serde_json::from_str(
            r#"{"action":"backup","serial":"ABC123","destination":"C:\\Backups","packages":["com.example.app"],"storage":["DCIM"]}"#,
        )
        .unwrap();
        assert_eq!(request.action, "backup");
        assert_eq!(request.packages, vec!["com.example.app"]);
        assert!(request.backup_private_data);
        assert!(request.backup_apk);
        let selective: Request = serde_json::from_str(
            r#"{"action":"backup","backup_apk":false,"backup_private_data":true}"#,
        )
        .unwrap();
        assert!(!selective.backup_apk);
        assert!(selective.backup_private_data);
    }
}
