# Android Backup

A Windows-native Android backup utility using **WinUI 3** for the interface and **Rust** for the backup engine.

**v0.3.0 preview**: WinUI 3 has replaced egui. An administrator UAC prompt is intentionally requested on every launch.

## Features

- Windows App SDK / WinUI 3 frontend with NavigationView, Mica, native dialogs and the Android Backup icon.
- Unpackaged, self-contained distribution (no separate Windows App SDK runtime installation).
- UAC manifest with requireAdministrator. Consent or administrator credentials are required before the app opens.
- Rust headless backup engine with a JSON Lines bridge, without CMD popups.
- Root verification via ADB, APK and split APK copying, available CE/DE app data and shared storage.
- SHA-256 verification of completed backups.
- One-click official Google Platform Tools download and installation into local app data.
- Driver INF selection and confirmation before elevated Windows PnPUtil installation.
- Optional link to Microsoft's VC++ Redistributable.

## Windows download

Open the latest successful **WinUI 3 Windows desktop** GitHub Actions run, download artifact
**android-backup-winui3-win-x64**, extract the archive and its inner ZIP, and keep
the complete AndroidBackup folder. Double-click AndroidBackup.exe and accept UAC.

The application ships as a self-contained folder, not the old single Rust GUI executable.
Do not copy AndroidBackup.exe away from its runtime dependencies and backend folder.

## Requirements

- Windows 10 build 19041+ or Windows 11, x64.
- Administrator consent at app launch (the user requested this behavior).
- A USB-debugging-authorized Android device, with temporary root already active.
- The device must expose the tar utility for streamed data archives.
- Enough trusted local disk space for unencrypted backups.

**Windows administrator rights are not Android root privileges.**
The phone's root shell must be obtained separately.

## Architecture

- windows/BackupAndroid.WinUI: C# WinUI 3 interface, UAC manifest, native driver setup
- src/main.rs: Rust JSON Lines backend entry point
- src/adb.rs: root detection and streaming
- src/backup.rs: APK/private data/shared media copying
- src/manifest.rs: SHA-256 integrity verification

The UI talks to the Rust engine using one JSON request on stdin and JSON Lines
status/progress events on stdout. Neither the UI nor the backend opens a visible CMD window.

## Ransomware indicator scan

Open **Ransomware scan** in WinUI 3 after connecting the device and verifying an active root shell.

- Uses Android ADB to inspect file **names only** in DCIM, Pictures, Movies, Download, Documents, Music and Android/media.
- Detects several known ransom-note naming patterns and suspicious encrypted-file extensions, without reading file contents or modifying data.
- Bounded to 2,500 filenames per available directory; truncated scans and inaccessible directories are reported.
- Shows low/medium/high heuristic indicator levels and up to 80 matching paths in the UI.
- **Not an antivirus engine.** An unmatched file can still be malicious, and a match may be harmless. A scan without findings is not proof that the device is safe.
- No automatic deletion, quarantine, network uploads, or claiming a malware family from filenames.
- A separate Google Play Protect scan remains useful for detecting potentially harmful Android apps.

## Security and limits

- Always-elevated applications have greater privileges and risk. A separate elevated
  driver helper is preferable in future revisions, but this preview requests elevation
  at startup as specified.
- Driver installation requires an explicitly selected OEM INF and user confirmation.
  It does not guess and force-install random vendor drivers.
- Backups are currently unencrypted and can contain private application data.
- Android encryption, Keystore-bound data, UID/GID mapping, signatures and SELinux
  restrictions mean some backups cannot be restored completely.
- Automatic restore and physical-device validation are not yet implemented.

## Build

The repository includes GitHub Actions for both Rust engine tests and a self-contained
Windows WinUI 3 application bundle.

Locally on Windows with the Windows SDK, .NET 8 SDK, and Rust installed:

    cargo build --release --bin backup-android-core
    dotnet publish windows/BackupAndroid.WinUI/BackupAndroid.WinUI.csproj -c Release -r win-x64 --self-contained true -p:Platform=x64

Place the Rust executable in a backend subfolder beside the published WinUI 3 application.
