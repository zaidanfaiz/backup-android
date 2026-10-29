# Android Backup

A Windows-native Android backup utility using **WinUI 3** for the interface and **Rust** for the backup engine.

**v0.3.2 preview**: Fixes a WinUI startup crash caused by a missing default theme dictionary, preserves the native launcher, and adds clearly separated Config, Language and Logs folders. Windows UAC remains required.

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

## Windows download and folder structure

Download the latest successful **WinUI 3 Windows desktop** artifact named
**AndroidBackup-WinUI3-Windows-x64**. Extract it once and run the top-level
`AndroidBackup.exe`. Windows will request UAC elevation.

```text
AndroidBackup/
├── AndroidBackup.exe        Windows launcher with app icon
├── README.txt
├── App/
│   ├── AndroidBackup.exe    Real WinUI 3 executable
│   ├── *.dll, *.pri         Required runtime dependencies
│   ├── Assets/              Application icons
│   ├── backend/             Rust backup engine
│   └── en-US/, ...          Framework language satellite resources
├── Config/                 Platform configuration notes
├── Language/               User documentation in English and Indonesian
└── Logs/                   Portable startup diagnostic log copies
```

The launcher reports missing runtime files and early WinUI crashes instead
of silently exiting. v0.3.2 also merges `XamlControlsResources` before custom
theme definitions in `App.xaml`, fixing the reported
`TabViewButtonBackground` lookup exception at window startup. Startup logs are stored in
`%LOCALAPPDATA%\\AndroidBackup\\Logs\\launcher.log` and
`%LOCALAPPDATA%\\AndroidBackup\\Logs\\startup.log`.

**Why the App folder contains DLL files:** WinUI 3 and native Windows
DLL loading require some DLLs next to the real application executable.
Renaming or moving them into an arbitrary DLL folder can prevent startup.
Instead, the release keeps the root directory clean and contains all WinUI
dependencies under `App/`; language files remain in their legitimate
locale-specific subfolders. The Language folder contains application-owned
user documentation and does not replace Windows framework satellite resources.
Portable logs are mirrored when writable; the per-user logs remain primary.

The release process validates the bundled Rust engine, icon and WinUI
runtime resources, and runs a launcher installation preflight.
A successful compile or packaging check does not prove it works on every PC.
If the app still fails to open, the included README explains how to collect
the startup log and Windows Application event code.

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
