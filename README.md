# Android Backup

A Windows-native Android backup utility using **WinUI 3** for the interface and **Rust** for the backup engine.

**v0.5.0 preview**: Reworked WinUI navigation with explicit vertical scrolling and independent component selection for rooted Android apps. The WinUI frontend and native Rust backup engine remain separate.

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

## Startup crash fixed in v0.3.3

One physical-device startup log reported `App.InitializeComponent: System.Runtime.InteropServices.COMException (0x8000FFFF)`. This came from the *diagnostic check* in `App.xaml.cs` that read `Resources.MergedDictionaries` before WinUI had finished application initialization. The check has been removed. The required `XamlControlsResources` entry remains in `App.xaml`.

Windows CI now verifies that the default control-resource dictionary is declared first, prohibits reintroducing the premature C# resource access, and attempts an interactive launch smoke test when its desktop environment permits. A CI build alone does not prove launch works on a user's own Windows installation.

Use a fresh extracted folder when testing. Logs remain in the portable `Logs/` directory when writable, or under `%LOCALAPPDATA%\\AndroidBackup\\Logs/`.

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

## v0.5.1: Restore preflight and safer execution

The **Restore app data** page now requires **Check app compatibility** after
choosing a package. This read-only step verifies the original Android device
and version, currently installed APK and split APK SHA-256 hashes, app data TAR
safety, availability of root restore tools, and selected-package integrity.
The Restore button stays disabled until the selected app passes all checks.
Switching applications or picking a different backup invalidates the check.

When restore starts, the Windows app **does not offer Cancel** because killing
the backend during root extraction could leave application files half-written.
Keep the device connected and the app open until completion. The mandatory
pre-restore safety backup remains in place. Restore is still experimental and
has not been validated on a physical rooted test device.

For performance, individual app preflights and final restores reverify only
the selected package's artifacts instead of hashing every app and media file
again. The initial backup folder preview still verifies the entire manifest.

## Restore installed application data (experimental)

The new **Restore app data** page handles the missing second half of the
backup workflow. It restores archives on **the original, already-rooted phone**:

1. Connect the original Android device, inspect it, and verify root.
2. Select a completed backup session directory containing `manifest.json`.
3. Preview available packages and check all SHA-256 digests, target serial,
   Android release, and backup completeness.
4. Select one package that is **already installed** on that device.
   The current base APK and split APKs must match their backed-up SHA-256
   hashes exactly before any private data is overwritten.
5. Confirm restore. A new verified safety backup of the app's current APK and
   data is created first in `Documents/AndroidBackups/restore-safety`.
6. The restore engine checks each TAR entry, rejects traversal, links and
   unexpected paths, then stages and restores the selected app's private CE/DE,
   Android/data, OBB and per-app media archives. It repairs UID/GID and SELinux
   labels on private app directories with Android `chown` and `restorecon`.

**Limitations:** this strict preview only restores the identical installed APK
build. App updates, missing APK backups or a different split-APK set block
restoration. Certificate-aware version migration is future work. The restore
is an in-place merge, not a transactional file
replacement. If an operation fails partway through, the safety backup is
retained and the current application may need manual repair. This is a
**same-device, same-Android-release, user-0 preview**. It does not reinstall
APK files, restore a different phone or ROM, or recover hardware-bound
Android Keystore secrets. The app must already be installed with the correct
package identity. TAR archives containing symlinks or special entries are
rejected rather than blindly extracted. No device data is touched by the
preview step.

The safety snapshot can contain passwords, session data and other private
information. Protect the entire backup directory. Do not publish it.

## App data backup workflow

The **Applications** page supports three presets:

- **Complete app backup:** APK and split APKs, credential-encrypted and device-encrypted private data, per-app Android/data, OBB and Android/media directories when accessible.
- **App and private data:** APK plus internal private data.
- **APK only:** APK and split APK files without private data.

Every selected package is stored separately beneath `apps/<package-name>/` in the backup session.
Private app data is archived as `credential_encrypted.tar` and `device_encrypted.tar`;
per-app external data is archived separately. Manifests include SHA-256 hashes
for all completed artifacts. A rooted and *unlocked* Android device is required
for credential-encrypted user data. App data is not the same as media folders.

**This release is backup-only.** A restore engine which safely handles UID/GID,
app signatures, SELinux labels, CE/DE encryption and Android Keystore data is
not implemented. Do not assume a copied private-data archive can be restored
on a different Android version or phone.

For behavioral reference, Swift Backup is a proprietary Android backup product;
the public GitHub projects [Neo Backup](https://github.com/NeoApplications/Neo-Backup)
and [DataBackup](https://github.com/XayahSuSuSu/Android-DataBackup) provide
open-source root-aware app backup approaches. Their code is not copied into
this repository and their respective licenses must be respected.

## Ransomware indicator scan

Open **Ransomware scan** in WinUI 3 after connecting the device and verifying an active root shell.

- Uses Android ADB to inspect file **names only** in DCIM, Pictures, Movies, Download, Documents, Music and Android/media.
- Detects several known ransom-note naming patterns and suspicious encrypted-file extensions, without reading file contents or modifying data.
- Bounded to 2,500 filenames per available directory; truncated scans and inaccessible directories are reported.
- Shows low/medium/high heuristic indicator levels and up to 80 matching paths in the UI.
- **Not an antivirus engine.** An unmatched file can still be malicious, and a match may be harmless. A scan without findings is not proof that the device is safe.
- No automatic deletion, quarantine, network uploads, or claiming a malware family from filenames.
- A separate Google Play Protect scan remains useful for detecting potentially harmful Android apps.

## App data backup components

The Windows Apps page now allows selecting the installed packages and choosing
which backup components to include, similar in concept to Swift Backup, Neo Backup,
and Android-DataBackup. This is an original implementation; it does not copy
Swift Backup's proprietary source or interface.

- **APK + splits**: base APK and configuration split APKs resolved from `pm path`.
- **Private app data (CE + DE)**: `/data/user/0/<package>` and
  `/data/user_de/0/<package>`, when the temporary root shell can read them.
- **External app data**: `/storage/emulated/0/Android/data/<package>`.
- **OBB / expansion**: `/storage/emulated/0/Android/obb/<package>`.
- **App media**: `/storage/emulated/0/Android/media/<package>`.
- **Shared folders**: DCIM, Pictures, Movies, Download, Documents and other
  explicitly selected locations on the Photos & Files page.

Selected apps survive filtering by package name. Each component is streamed to
a per-package directory and recorded in `manifest.json` with a SHA-256 digest.
Files are not automatically restored. Android Keystore and unavailable encrypted
data may prevent complete account/session migration.

Scrolling is enabled explicitly on all five navigation pages. UI events are
marshalled to the WinUI UI thread, Android device changes invalidate stale root
status, and the Windows build runs a GUI navigation and scrolling smoke test.

The current release does **not** back up SMS, call history, or Wi-Fi
configurations and does not provide the full Swift Backup feature set. It
requires actual root on the Android device to read private data.

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
