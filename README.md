# Android Backup

Native, local-first Android backup desktop application written in Rust.

This project uses Android Debug Bridge (ADB) and an **already-active root shell** to back up installed APKs, private app data and shared storage to a desktop computer. Root access is a hard prerequisite. The tool does not root or unlock Android devices.

**Status: v0.1.1 development.** Backup and SHA-256 verification are implemented. Automatic restore, encryption, compression and user-profile migration are not yet implemented.

## Features

- Rust-native desktop UI powered by egui / eframe.
- Device discovery, device selection and effective UID 0 validation.
- Works with existing su root access or root-enabled adbd. Temp root must be activated separately.
- Discovers third-party applications for Android user 0.
- Exports the base APK and split APKs returned by Android package manager.
- Backs up available credential-encrypted and device-encrypted app folders.
- Includes per-app external files, OBB and media when present.
- Selectable DCIM, Pictures, Movies, Download, Documents, Music, Android/media, Android/data and Android/obb.
- Optional force-stop before copying each app.
- Binary-safe ADB streaming without staging full archives on the phone.
- JSON manifest with file size and SHA-256 digests.
- Existing-backup verification, cancellation, incomplete-backup detection.

## Requirements

1. Windows 10/11 or supported Linux desktop. macOS has not been tested yet.
2. Rust stable toolchain for building from source, and Windows SDK with rc.exe when compiling on Windows.
3. Android SDK Platform Tools installed. Set the full adb.exe path if it is not on PATH.
4. USB debugging enabled and authorized, preferably with a USB cable.
5. Already-active temporary root or another working UID 0 shell. The application verifies UID 0, not whether that root originated from a temporary exploit.
6. Working tar executable on Android, sufficient computer disk space.
7. Unlock Android's primary user after boot to access credential-encrypted data.

This application does not rely on the restricted Android adb backup format. It uses adb exec-out and the already-authorized root shell.

## Windows portable executable

Download the latest successful GitHub Actions artifact named **android-backup-windows-x64-portable**. Extract it, then double-click **backup-android.exe**.

- This is a native Windows GUI application and does **not** launch a command prompt. ADB subprocesses are run with hidden console windows.
- The executable contains the Android Backup icon for Windows Explorer, and the same icon appears in its window/taskbar.
- The Windows/MSVC build statically links the C runtime, avoiding the missing `VCRUNTIME140.dll` startup error seen in the older developer build.
- Android SDK Platform Tools (ADB) must still be available in PATH, or configure its full path in the application's Connection section.
- The portable executable is not code-signed. Windows SmartScreen may warn about a new or unknown publisher.

The icon is authored in `assets/icon.svg` with companion `.ico` (Windows resources) and raw RGBA (window icon) assets. The generated release executable is available from the Actions run artifacts, not from old build downloads.

## Build and run

    git clone https://github.com/zaidanfaiz/backup-android.git
    cd backup-android
    cargo run --release

Once launched:

1. Connect the device and authorize its USB debugging prompt.
2. Click Refresh devices, choose the device, then Inspect device & root.
3. Ensure the app reports Root verified. If root is unavailable, obtain temp root separately and inspect again.
4. Select installed apps and shared-storage categories.
5. Choose a private local destination directory.
6. Optionally force-stop selected apps for more consistent app database copies.
7. Start backup. You can run Verify existing backup to check a saved directory again.

## Example output

    AndroidBackups/
      android-SERIAL-YYYYMMDD-HHMMSS-SSS/
        manifest.json
        apps/
          com.example.app/
            apk/
              00-base.apk
              01-split_config.arm64_v8a.apk
            credential_encrypted.tar
            device_encrypted.tar
            external_data.tar
            obb.tar
            media.tar
        storage/
          DCIM.tar
          Pictures.tar
          Android_data.tar

Only selected categories and paths found on the device are included. The actual APK names vary. Android app backup files and shared-storage content can be sensitive and may consume considerable disk space.

The manifest stores device details, package selections, artifacts, SHA-256 digests and a completion flag. Failed/cancelled sessions leave the flag false. TAR archives are uncompressed in this version.

## Technical and security limitations

- **Automatic restore is not implemented.** Restoring private app data on another Android version, UID, signature or device may fail. SELinux contexts and hardware-bound Keystore keys can also prevent successful restoration.
- **Not a guaranteed full-system image.** Locked profiles, encryption, volatile root access, running databases and inaccessible partitions may leave some data unavailable.
- **User 0, third-party apps only** in the current app picker. Work profiles, other Android users and system apps are not selectable yet.
- **Not an atomic snapshot.** Apps can change data during copying. Optional force-stop helps, but does not create a filesystem snapshot.
- **Plaintext data at rest.** There is no backup encryption yet. Private data, sessions and credentials may be contained in the output. Never upload your backup to a public repository.
- **Do not treat partial backups as complete.** Published artifacts are hashed and locally verified at completion. Incomplete backups cannot pass verification.
- **Temp root can expire.** Backup stops on a detected ADB/root command error. Refresh root if necessary.
- **No automated device-rooting, activation bypass or exploit code.**

## Source layout

    src/main.rs        Desktop application entry point
    src/ui.rs          UI state and background worker events
    src/adb.rs         ADB execution, root checks and streaming
    src/backup.rs      APK, app data and shared-storage orchestration
    src/manifest.rs    Backup manifest and SHA-256 verification

Lengthy ADB operations run on worker threads instead of blocking the UI. Transfers are written to temporary .part files before being committed. Android package names are validated and device-provided paths are shell-quoted.

## Roadmap

- Preflight space estimation and detailed per-artifact diagnostics
- Compression and authenticated encryption
- Safe, version-aware restore flow and validation
- Multi-user / work profile support
- Android emulator and device integration tests
- Signed desktop release builds

## Responsible use

Use this tool only on Android devices or profiles that you own or have explicit authorization to manage. A working root session exposes sensitive user data. Backups must be stored and handled responsibly.
