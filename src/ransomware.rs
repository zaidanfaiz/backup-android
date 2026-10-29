//! Read-only ransomware *indicator* scanner for Android shared storage.
//!
//! This is NOT an antivirus engine or a guarantee that a device is clean.
//! It checks a bounded set of filenames, never opens file contents, and never
//! modifies Android storage. Matches are heuristic and may be false positives.
use crate::adb::{shell_quote, Adb};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashSet;

const ROOT: &str = "/storage/emulated/0";
const DIRECTORIES: &[&str] = &[
    "DCIM",
    "Pictures",
    "Movies",
    "Download",
    "Documents",
    "Music",
    "Android/media",
];
const MAX_PER_DIRECTORY: usize = 2500;
const MAX_REPORTED_FINDINGS: usize = 80;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    High,
    Medium,
    Low,
    None,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Indicator {
    RansomNoteName,
    SuspiciousExtension,
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub indicator: Indicator,
    pub path: String,
    pub explanation: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ScanReport {
    pub severity: Severity,
    pub files_examined: usize,
    pub directories_examined: usize,
    pub ransom_note_matches: usize,
    pub suspicious_extension_matches: usize,
    pub truncated: bool,
    pub findings: Vec<Finding>,
    pub warnings: Vec<String>,
    pub message: &'static str,
}

fn marker(path: &str) -> Option<(Indicator, &'static str)> {
    let file = path.rsplit('/').next()?.to_ascii_lowercase();

    // Deliberately avoid matching ordinary README.txt or general encryption
    // formats such as .enc/.gpg/.zip, which have many legitimate uses.
    const NOTES: &[&str] = &[
        "how_to_decrypt.txt",
        "how_to_decrypt.html",
        "decrypt_instructions.txt",
        "readme_for_decrypt.txt",
        "readme_to_decrypt.txt",
        "your_files_are_encrypted.txt",
        "recover_your_files.txt",
        "_readme.txt",
        "!readme!.txt",
    ];
    if NOTES.contains(&file.as_str()) {
        return Some((
            Indicator::RansomNoteName,
            "Filename resembles a ransomware payment or recovery note",
        ));
    }

    const EXTENSIONS: &[&str] = &[
        ".locked",
        ".encrypted",
        ".crypt",
        ".locky",
        ".zepto",
        ".cerber",
        ".wncry",
    ];
    if EXTENSIONS.iter().any(|extension| file.ends_with(extension)) {
        return Some((
            Indicator::SuspiciousExtension,
            "Filename ends with an extension sometimes seen after encryption",
        ));
    }
    None
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(320)
        .collect()
}

/// Scan only standard Android shared-storage directories. No app data, file
/// content or remote network services are accessed.
pub fn scan_device(
    adb: &Adb,
    mut on_progress: impl FnMut(String),
) -> Result<ScanReport> {
    adb.check_ready()?;

    let mut report = ScanReport {
        severity: Severity::None,
        files_examined: 0,
        directories_examined: 0,
        ransom_note_matches: 0,
        suspicious_extension_matches: 0,
        truncated: false,
        findings: Vec::new(),
        warnings: Vec::new(),
        message: "No filename indicators were found in the sampled folders; this does not prove the device is clean.",
    };
    let mut found = HashSet::new();

    for relative in DIRECTORIES {
        let directory = format!("{ROOT}/{relative}");
        if !adb.directory_exists(&directory)? {
            report.warnings.push(format!("{relative}: directory does not exist"));
            continue;
        }
        on_progress(format!("Checking {relative}..."));

        // Limit the work per directory. We do not recurse through Android/data,
        // credential stores or other app-private files.
        let command = format!(
            "find {} -type f -print 2>/dev/null | head -n {}",
            shell_quote(&directory),
            MAX_PER_DIRECTORY + 1
        );
        let output = match adb.root_text(&command) {
            Ok(text) => text,
            Err(error) => {
                report.warnings.push(format!(
                    "{relative}: listing failed ({error:#})"
                ));
                continue;
            }
        };
        report.directories_examined += 1;
        let mut lines = output.lines();
        for _ in 0..MAX_PER_DIRECTORY {
            let Some(raw) = lines.next() else { break };
            let path = raw.trim();
            if !path.starts_with(&format!("{directory}/")) {
                continue;
            }
            report.files_examined += 1;
            if let Some((indicator, explanation)) = marker(path) {
                if !found.insert(path.to_owned()) {
                    continue;
                }
                match indicator {
                    Indicator::RansomNoteName => report.ransom_note_matches += 1,
                    Indicator::SuspiciousExtension => report.suspicious_extension_matches += 1,
                };
                if report.findings.len() < MAX_REPORTED_FINDINGS {
                    report.findings.push(Finding {
                        indicator,
                        path: safe_name(path),
                        explanation,
                    });
                }
            }
        }
        if lines.next().is_some() {
            report.truncated = true;
            report.warnings.push(format!(
                "{relative}: only the first {MAX_PER_DIRECTORY} filenames were examined"
            ));
        }
    }

    // A filename alone is not proof of malware. Correlated note and extension
    // indicators merit investigation; neither causes automatic file deletion.
    report.severity = if report.ransom_note_matches > 0
        && report.suspicious_extension_matches >= 5
    {
        Severity::High
    } else if report.ransom_note_matches > 0 || report.suspicious_extension_matches >= 5 {
        Severity::Medium
    } else if report.suspicious_extension_matches > 0 {
        Severity::Low
    } else {
        Severity::None
    };
    report.message = match report.severity {
        Severity::High => "Multiple correlated ransomware indicators detected. Preserve evidence and investigate.",
        Severity::Medium => "Suspicious filenames detected. Review them before making changes.",
        Severity::Low => "Some file extensions need review; legitimate files can match these patterns.",
        Severity::None => "No filename indicators were found in the sampled folders; this does not prove the device is clean.",
    };
    if report.directories_examined == 0 {
        report.warnings.push("No accessible shared-storage directories were checked".to_owned());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_likely_ransom_note_name() {
        assert_eq!(
            marker("/storage/emulated/0/Download/HOW_TO_DECRYPT.txt")
                .unwrap()
                .0,
            Indicator::RansomNoteName
        );
    }

    #[test]
    fn does_not_flag_normal_encrypted_files_or_readmes() {
        for name in ["README.txt", "notes.gpg", "backup.enc", "document.pdf", "photo.jpg"] {
            assert!(marker(&format!("/storage/emulated/0/{name}")).is_none());
        }
    }

    #[test]
    fn classifies_encrypted_extension() {
        assert_eq!(
            marker("/storage/emulated/0/DCIM/image.jpg.locked")
                .unwrap()
                .0,
            Indicator::SuspiciousExtension
        );
    }

    #[test]
    fn redacts_control_characters_from_display() {
        assert_eq!(safe_name("file\nname\0.txt"), "filename.txt");
    }
}
