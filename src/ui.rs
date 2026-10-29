use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::Duration,
};

use crate::{
    adb::{self, Adb, Device},
    backup::{self, BackupOptions, Progress, STORAGE_CATEGORIES},
    manifest,
};

struct PackageRow {
    name: String,
    checked: bool,
}

struct StorageRow {
    path: &'static str,
    label: &'static str,
    checked: bool,
}

enum Event {
    Devices(Result<Vec<String>, String>),
    Inspected(Result<(Device, Vec<String>), String>),
    Progress(Progress),
    BackupFinished(Result<PathBuf, String>),
    VerificationFinished(Result<usize, String>),
}

pub struct BackupApp {
    adb_path: String,
    devices: Vec<String>,
    selected_serial: String,
    device: Option<Device>,
    packages: Vec<PackageRow>,
    storage: Vec<StorageRow>,
    output_dir: String,
    force_stop: bool,
    busy: bool,
    backing_up: bool,
    cancel: Arc<AtomicBool>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    logs: Vec<String>,
    current_file: String,
    transferred_bytes: u64,
}

impl BackupApp {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let default_output = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("AndroidBackups");

        Self {
            adb_path: "adb".into(),
            devices: Vec::new(),
            selected_serial: String::new(),
            device: None,
            packages: Vec::new(),
            storage: STORAGE_CATEGORIES
                .iter()
                .map(|(path, label)| StorageRow {
                    path,
                    label,
                    checked: matches!(
                        *path,
                        "DCIM" | "Pictures" | "Movies" | "Download" | "Documents"
                    ),
                })
                .collect(),
            output_dir: default_output.to_string_lossy().into_owned(),
            force_stop: false,
            busy: false,
            backing_up: false,
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            logs: vec!["Ready. Connect a USB-debugging-enabled Android device.".into()],
            current_file: String::new(),
            transferred_bytes: 0,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.logs.push(message.into());
        if self.logs.len() > 250 {
            self.logs.drain(..self.logs.len() - 250);
        }
    }

    fn collect_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Devices(Ok(devices)) => {
                    self.busy = false;
                    self.log(format!("Found {} authorized device(s)", devices.len()));
                    self.devices = devices;
                    if !self.devices.contains(&self.selected_serial) {
                        self.selected_serial = self.devices.first().cloned().unwrap_or_default();
                    }
                    self.device = None;
                    self.packages.clear();
                }
                Event::Devices(Err(error)) => {
                    self.busy = false;
                    self.log(format!("Device scan failed: {error}"));
                }
                Event::Inspected(Ok((device, packages))) => {
                    self.busy = false;
                    if let Some(root) = device.root {
                        self.log(format!("Root access: {}", root.label()));
                    } else {
                        self.log("No active root shell. Activate temp root first.");
                    }
                    self.log(format!("{} installed user apps found", packages.len()));
                    self.packages = packages
                        .into_iter()
                        .map(|name| PackageRow {
                            name,
                            checked: true,
                        })
                        .collect();
                    self.device = Some(device);
                }
                Event::Inspected(Err(error)) => {
                    self.busy = false;
                    self.device = None;
                    self.packages.clear();
                    self.log(format!("Device inspection failed: {error}"));
                }
                Event::Progress(Progress::Message(message)) => self.log(message),
                Event::Progress(Progress::Transferred { file, bytes }) => {
                    self.current_file = file;
                    self.transferred_bytes = bytes;
                }
                Event::BackupFinished(result) => {
                    self.busy = false;
                    self.backing_up = false;
                    self.current_file.clear();
                    match result {
                        Ok(folder) => {
                            self.log(format!("Backup completed: {}", folder.display()));
                        }
                        Err(error) => self.log(format!(
                            "Backup failed: {error:#}. Existing artifacts are incomplete."
                        )),
                    }
                }
                Event::VerificationFinished(result) => {
                    self.busy = false;
                    match result {
                        Ok(count) => self.log(format!("Verification passed: {count} files")),
                        Err(error) => self.log(format!("Verification failed: {error:#}")),
                    }
                }
            }
        }
    }

    fn refresh_devices(&mut self) {
        self.busy = true;
        let adb_path = self.adb_path.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result =
                adb::list_devices(Path::new(&adb_path)).map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::Devices(result));
        });
    }

    fn inspect_device(&mut self) {
        if self.selected_serial.is_empty() {
            self.log("No authorized device selected.");
            return;
        }
        self.busy = true;
        let binary = PathBuf::from(self.adb_path.clone());
        let serial = self.selected_serial.clone();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = (|| {
                let device = adb::inspect(&binary, &serial)?;
                let packages = if device.root.is_some() {
                    Adb::new(binary, &device)?.packages()?
                } else {
                    Vec::new()
                };
                Ok((device, packages))
            })()
            .map_err(|error: anyhow::Error| format!("{error:#}"));
            let _ = tx.send(Event::Inspected(result));
        });
    }

    fn begin_backup(&mut self) {
        let Some(device) = self.device.clone() else {
            self.log("Inspect your device and verify root first.");
            return;
        };
        if device.root.is_none() {
            self.log("Backup blocked: root access is required.");
            return;
        }
        let options = BackupOptions {
            adb_path: PathBuf::from(&self.adb_path),
            device,
            destination: PathBuf::from(&self.output_dir),
            packages: self
                .packages
                .iter()
                .filter(|row| row.checked)
                .map(|row| row.name.clone())
                .collect(),
            storage: self
                .storage
                .iter()
                .filter(|row| row.checked)
                .map(|row| row.path.to_owned())
                .collect(),
            force_stop: self.force_stop,
        };
        if options.packages.is_empty() && options.storage.is_empty() {
            self.log("Choose at least one app or storage category.");
            return;
        }

        self.cancel.store(false, Ordering::Relaxed);
        self.busy = true;
        self.backing_up = true;
        self.transferred_bytes = 0;
        self.current_file.clear();
        self.log("Starting backup...");
        let tx = self.tx.clone();
        let cancel = Arc::clone(&self.cancel);
        thread::spawn(move || {
            let result = backup::run_backup(options, &cancel, |progress| {
                let _ = tx.send(Event::Progress(progress));
            })
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::BackupFinished(result));
        });
    }

    fn verify_existing(&mut self) {
        let Some(folder) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.busy = true;
        self.log(format!("Verifying {}", folder.display()));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = manifest::verify_backup(&folder).map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::VerificationFinished(result));
        });
    }

    fn setup_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Connection");
        ui.label("ADB executable");
        ui.add_enabled(
            !self.busy,
            egui::TextEdit::singleline(&mut self.adb_path).hint_text("adb or full adb.exe path"),
        );
        if ui
            .add_enabled(!self.busy, egui::Button::new("Refresh devices"))
            .clicked()
        {
            self.refresh_devices();
        }

        ui.horizontal(|ui| {
            ui.label("Device");
            egui::ComboBox::from_id_salt("serial_selector")
                .selected_text(if self.selected_serial.is_empty() {
                    "No devices"
                } else {
                    &self.selected_serial
                })
                .show_ui(ui, |ui| {
                    for serial in &self.devices {
                        ui.selectable_value(&mut self.selected_serial, serial.clone(), serial);
                    }
                });
        });
        if self
            .device
            .as_ref()
            .is_some_and(|device| device.serial != self.selected_serial)
        {
            self.device = None;
            self.packages.clear();
        }
        if ui
            .add_enabled(
                !self.busy && !self.selected_serial.is_empty(),
                egui::Button::new("Inspect device & root"),
            )
            .clicked()
        {
            self.inspect_device();
        }
        if let Some(device) = &self.device {
            ui.label(format!(
                "{} | Android {}",
                device.model, device.android_version
            ));
            if let Some(root) = device.root {
                ui.colored_label(
                    egui::Color32::from_rgb(86, 186, 126),
                    format!("Root verified: {}", root.label()),
                );
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(224, 116, 94),
                    "No root shell. Backup unavailable.",
                );
            }
        }

        ui.separator();
        ui.heading("Destination");
        ui.add_enabled(
            !self.busy,
            egui::TextEdit::singleline(&mut self.output_dir)
                .hint_text("Backup destination directory"),
        );
        if ui
            .add_enabled(!self.busy, egui::Button::new("Choose folder"))
            .clicked()
        {
            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                self.output_dir = folder.to_string_lossy().into_owned();
            }
        }

        ui.separator();
        ui.heading("Shared storage");
        ui.small("App-private data is backed up separately.");
        for row in &mut self.storage {
            ui.add_enabled_ui(!self.busy, |ui| {
                ui.checkbox(&mut row.checked, row.label);
            });
        }

        ui.separator();
        ui.add_enabled_ui(!self.busy, |ui| {
            ui.checkbox(
                &mut self.force_stop,
                "Force-stop selected apps before backup",
            );
        });
        ui.small("Recommended for database consistency. May interrupt app activity.");

        ui.add_space(8.0);
        let allowed = !self.busy
            && self
                .device
                .as_ref()
                .is_some_and(|device| device.root.is_some());
        if ui
            .add_enabled(allowed, egui::Button::new("Start backup"))
            .clicked()
        {
            self.begin_backup();
        }
        if self.backing_up && ui.button("Cancel backup").clicked() {
            self.cancel.store(true, Ordering::Relaxed);
            self.log("Cancellation requested. Finishing the current I/O operation...");
        }
        if ui
            .add_enabled(!self.busy, egui::Button::new("Verify existing backup"))
            .clicked()
        {
            self.verify_existing();
        }
        if self.busy {
            ui.spinner();
        }
        ui.small("Local backups are unencrypted. Keep the destination private.");
    }

    fn content_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Applications");
        ui.label("Select installed user apps. Each includes APK splits and available data.");
        let selected = self.packages.iter().filter(|row| row.checked).count();
        ui.horizontal(|ui| {
            ui.label(format!("{selected} / {} selected", self.packages.len()));
            if ui
                .add_enabled(!self.busy, egui::Button::new("Select all"))
                .clicked()
            {
                for row in &mut self.packages {
                    row.checked = true;
                }
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Clear"))
                .clicked()
            {
                for row in &mut self.packages {
                    row.checked = false;
                }
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("packages")
            .max_height(270.0)
            .show(ui, |ui| {
                if self.packages.is_empty() {
                    ui.weak("Inspect a rooted device to load installed applications.");
                }
                for row in &mut self.packages {
                    ui.add_enabled_ui(!self.busy, |ui| {
                        ui.checkbox(&mut row.checked, &row.name);
                    });
                }
            });

        ui.separator();
        ui.heading("Activity");
        if !self.current_file.is_empty() {
            ui.label(format!(
                "{}: {:.1} MiB transferred",
                self.current_file,
                self.transferred_bytes as f64 / (1024.0 * 1024.0)
            ));
        }
        egui::ScrollArea::vertical()
            .id_salt("activity")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for message in &self.logs {
                    ui.monospace(message);
                }
            });
    }
}

impl eframe::App for BackupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.collect_events();
        ctx.set_visuals(egui::Visuals::dark());
        egui::TopBottomPanel::top("title").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Android Backup");
                ui.separator();
                ui.weak("Rust desktop | Root required | Local-first");
            });
        });
        egui::SidePanel::left("setup")
            .min_width(290.0)
            .default_width(330.0)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.setup_panel(ui));
            });
        egui::CentralPanel::default().show(ctx, |ui| self.content_panel(ui));
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}
