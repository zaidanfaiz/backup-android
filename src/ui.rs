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
    manifest, setup,
};

const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(15, 21, 31);
const SIDEBAR: egui::Color32 = egui::Color32::from_rgb(21, 29, 41);
const CARD: egui::Color32 = egui::Color32::from_rgb(28, 38, 52);
const BORDER: egui::Color32 = egui::Color32::from_rgb(45, 58, 74);
const ACTIVE_NAV: egui::Color32 = egui::Color32::from_rgb(37, 58, 68);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(84, 220, 198);
const ACCENT_DARK: egui::Color32 = egui::Color32::from_rgb(30, 110, 101);
const TEXT: egui::Color32 = egui::Color32::from_rgb(237, 242, 248);
const MUTED: egui::Color32 = egui::Color32::from_rgb(164, 179, 192);
const WARNING: egui::Color32 = egui::Color32::from_rgb(240, 177, 110);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Applications,
    Media,
    Setup,
}

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
    ToolsInstalled(Result<PathBuf, String>),
}

pub struct BackupApp {
    page: Page,
    logo: egui::TextureHandle,
    app_filter: String,
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
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [64, 64],
            include_bytes!("../assets/icon.rgba"),
        );
        let logo =
            cc.egui_ctx
                .load_texture("android_backup_brand", image, egui::TextureOptions::LINEAR);
        let mut style = (*cc.egui_ctx.style()).clone();
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = BACKGROUND;
        visuals.window_fill = CARD;
        visuals.widgets.inactive.bg_fill = CARD;
        visuals.widgets.hovered.bg_fill = ACTIVE_NAV;
        visuals.selection.bg_fill = ACCENT_DARK;
        visuals.override_text_color = Some(TEXT);
        style.visuals = visuals;
        style.spacing.item_spacing = egui::vec2(9.0, 9.0);
        style.spacing.button_padding = egui::vec2(13.0, 9.0);
        cc.egui_ctx.set_style(style);
        let (tx, rx) = mpsc::channel();
        let default_output = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("AndroidBackups");

        Self {
            page: Page::Overview,
            logo,
            app_filter: String::new(),
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
                Event::ToolsInstalled(result) => {
                    self.busy = false;
                    match result {
                        Ok(path) => {
                            self.adb_path = path.to_string_lossy().into_owned();
                            self.log(format!(
                                "Android Platform Tools installed: {}",
                                path.display()
                            ));
                        }
                        Err(error) => self.log(format!("Platform Tools import failed: {error}")),
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

    fn heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
        ui.label(egui::RichText::new(title).size(27.0).strong().color(TEXT));
        ui.label(egui::RichText::new(subtitle).size(13.5).color(MUTED));
        ui.add_space(18.0);
    }

    fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new()
            .fill(CARD)
            .stroke(egui::Stroke::new(1.0_f32, BORDER))
            .corner_radius(egui::CornerRadius::same(14))
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_min_width((ui.available_width() - 2.0).max(0.0));
                content(ui);
            });
        ui.add_space(12.0);
    }

    fn section(ui: &mut egui::Ui, heading: &str, description: &str) {
        ui.label(egui::RichText::new(heading).size(17.0).strong().color(TEXT));
        ui.label(egui::RichText::new(description).color(MUTED));
        ui.add_space(12.0);
    }

    fn pill(ui: &mut egui::Ui, label: &str, positive: bool) {
        let color = if positive { ACCENT } else { WARNING };
        egui::Frame::new()
            .fill(if positive {
                egui::Color32::from_rgb(22, 69, 64)
            } else {
                egui::Color32::from_rgb(77, 53, 40)
            })
            .corner_radius(egui::CornerRadius::same(16))
            .inner_margin(egui::Margin::symmetric(11, 5))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(label).color(color).strong());
            });
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        ui.add_space(22.0);
        ui.label(
            egui::RichText::new("WORKSPACE")
                .color(MUTED)
                .size(11.0)
                .strong(),
        );
        ui.add_space(10.0);

        for (page, title) in [
            (Page::Overview, "Overview"),
            (Page::Applications, "Applications"),
            (Page::Media, "Photos & files"),
            (Page::Setup, "Setup & drivers"),
        ] {
            let selected = self.page == page;
            let button =
                egui::Button::new(egui::RichText::new(title).size(15.0).color(if selected {
                    TEXT
                } else {
                    MUTED
                }))
                .fill(if selected { ACTIVE_NAV } else { SIDEBAR })
                .stroke(egui::Stroke::NONE)
                .corner_radius(egui::CornerRadius::same(10));
            if ui.add_sized([188.0, 43.0], button).clicked() {
                self.page = page;
            }
            ui.add_space(4.0);
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("CONNECTION")
                .color(MUTED)
                .size(11.0)
                .strong(),
        );
        ui.add_space(8.0);
        if let Some(device) = &self.device {
            ui.label(egui::RichText::new(&device.model).color(TEXT).strong());
            ui.label(
                egui::RichText::new(format!("Android {}", device.android_version)).color(MUTED),
            );
            ui.add_space(6.0);
            Self::pill(
                ui,
                if device.root.is_some() {
                    "Root ready"
                } else {
                    "Root unavailable"
                },
                device.root.is_some(),
            );
        } else {
            ui.label(egui::RichText::new("No device inspected").color(MUTED));
        }
        ui.add_space(18.0);
        ui.label(
            egui::RichText::new("v0.2.0  ·  Local backup")
                .color(MUTED)
                .size(11.0),
        );
    }

    fn connection_card(&mut self, ui: &mut egui::Ui) {
        Self::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("Android device")
                            .size(18.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        egui::RichText::new("Connect with USB debugging enabled").color(MUTED),
                    );
                });
                ui.add_space(10.0);
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Refresh devices"))
                    .clicked()
                {
                    self.refresh_devices();
                }
            });
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.label("Selected device");
                egui::ComboBox::from_id_salt("device")
                    .selected_text(if self.selected_serial.is_empty() {
                        "No authorized devices"
                    } else {
                        &self.selected_serial
                    })
                    .show_ui(ui, |ui| {
                        for serial in &self.devices {
                            ui.selectable_value(&mut self.selected_serial, serial.clone(), serial);
                        }
                    });
                if ui
                    .add_enabled(
                        !self.busy && !self.selected_serial.is_empty(),
                        egui::Button::new("Check device & root").fill(ACCENT_DARK),
                    )
                    .clicked()
                {
                    self.inspect_device();
                }
            });
            if self
                .device
                .as_ref()
                .is_some_and(|d| d.serial != self.selected_serial)
            {
                self.device = None;
                self.packages.clear();
            }
            ui.add_space(10.0);
            if let Some(device) = &self.device {
                ui.horizontal(|ui| {
                    Self::pill(
                        ui,
                        if device.root.is_some() {
                            "UID 0 verified"
                        } else {
                            "Root required"
                        },
                        device.root.is_some(),
                    );
                    ui.label(
                        egui::RichText::new(format!(
                            "{}  |  Android {}",
                            device.model, device.android_version
                        ))
                        .color(MUTED),
                    );
                });
            } else {
                ui.label(
                    egui::RichText::new(
                        "No device verified yet. Check connection or open Setup & drivers.",
                    )
                    .color(MUTED),
                );
            }
        });
    }

    fn overview_panel(&mut self, ui: &mut egui::Ui) {
        Self::heading(
            ui,
            "Backup overview",
            "A safe, local-first backup of applications and files.",
        );
        self.connection_card(ui);

        let apps = self.packages.iter().filter(|app| app.checked).count();
        let folders = self.storage.iter().filter(|item| item.checked).count();
        Self::card(ui, |ui| {
            Self::section(
                ui,
                "What will be saved",
                "Your selections are stored locally in this session.",
            );
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(apps.to_string())
                            .size(30.0)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.label(egui::RichText::new("selected apps").color(MUTED));
                    if ui.link("Choose apps").clicked() {
                        self.page = Page::Applications;
                    }
                });
                ui.add_space(54.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(folders.to_string())
                            .size(30.0)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.label(egui::RichText::new("media folders").color(MUTED));
                    if ui.link("Choose folders").clicked() {
                        self.page = Page::Media;
                    }
                });
            });
        });

        Self::card(ui, |ui| {
            Self::section(
                ui,
                "Backup location",
                "Choose a private folder with enough available space.",
            );
            ui.horizontal(|ui| {
                ui.add_enabled(
                    !self.busy,
                    egui::TextEdit::singleline(&mut self.output_dir)
                        .desired_width(ui.available_width() - 90.0),
                );
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Browse"))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.output_dir = path.to_string_lossy().into_owned();
                    }
                }
            });
            ui.add_space(6.0);
            ui.add_enabled_ui(!self.busy, |ui| {
                ui.checkbox(
                    &mut self.force_stop,
                    "Force-stop apps for better data consistency",
                );
            });
            ui.label(
                egui::RichText::new(
                    "Archives are unencrypted in this preview. Store them securely.",
                )
                .color(WARNING)
                .size(12.0),
            );
        });

        Self::card(ui, |ui| {
            Self::section(
                ui,
                "Recent activity",
                "Connection, backup and verification messages.",
            );
            if !self.current_file.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  ·  {:.1} MiB",
                        self.current_file,
                        self.transferred_bytes as f64 / 1_048_576.0
                    ))
                    .color(ACCENT),
                );
                ui.add_space(8.0);
            }
            for entry in self.logs.iter().rev().take(5) {
                ui.label(egui::RichText::new(entry).color(MUTED).size(12.5));
            }
        });
    }

    fn applications_panel(&mut self, ui: &mut egui::Ui) {
        Self::heading(
            ui,
            "Applications",
            "Copy APKs, split APKs and accessible private app data.",
        );
        Self::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} of {} selected",
                    self.packages.iter().filter(|row| row.checked).count(),
                    self.packages.len()
                ));
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Select all"))
                    .clicked()
                {
                    for app in &mut self.packages {
                        app.checked = true;
                    }
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Clear"))
                    .clicked()
                {
                    for app in &mut self.packages {
                        app.checked = false;
                    }
                }
            });
            ui.add_space(10.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.app_filter)
                    .hint_text("Search package names")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(10.0);
            egui::ScrollArea::vertical().id_salt("app_picker").max_height(480.0).show(ui, |ui| {
                if self.packages.is_empty() {
                    ui.label(egui::RichText::new("Connect and inspect a rooted Android device to load installed apps.").color(MUTED));
                }
                for app in &mut self.packages {
                    if app.name.to_ascii_lowercase().contains(&self.app_filter.to_ascii_lowercase()) {
                        ui.add_enabled_ui(!self.busy, |ui| {
                            ui.checkbox(&mut app.checked, &app.name);
                        });
                    }
                }
            });
        });
    }

    fn media_panel(&mut self, ui: &mut egui::Ui) {
        Self::heading(
            ui,
            "Photos & files",
            "Select the shared storage directories to include.",
        );
        Self::card(ui, |ui| {
            Self::section(
                ui,
                "Internal shared storage",
                "Folders are streamed from the phone to the desktop.",
            );
            egui::Grid::new("storage_categories")
                .num_columns(2)
                .spacing([30.0, 12.0])
                .show(ui, |ui| {
                    for (index, category) in self.storage.iter_mut().enumerate() {
                        ui.add_enabled_ui(!self.busy, |ui| {
                            ui.checkbox(&mut category.checked, category.label);
                        });
                        if index % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
        });
        Self::card(ui, |ui| {
            Self::section(
                ui,
                "How app data works",
                "App-private storage is separate from photos and shared files.",
            );
            ui.label(egui::RichText::new("App data is read from accessible /data/user/0 and /data/user_de/0 locations after root validation. Device encryption, Keystore-backed content and other profiles may not be recoverable.").color(MUTED));
        });
    }

    fn link_button(&mut self, ui: &mut egui::Ui, text: &str, url: &'static str) {
        if ui.button(text).clicked() {
            if let Err(error) = setup::open_link(url) {
                self.log(format!("Could not open browser: {error:#}"));
            }
        }
    }

    fn import_tools(&mut self) {
        let selected = rfd::FileDialog::new()
            .add_filter("Android Platform Tools", &["zip"])
            .pick_file();
        let Some(path) = selected else {
            return;
        };
        self.busy = true;
        self.log(format!("Installing Platform Tools from {}", path.display()));
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = setup::install_platform_tools(&path).map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::ToolsInstalled(result));
        });
    }

    fn install_inf(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Windows driver", &["inf"])
            .pick_file()
        {
            match setup::install_driver_inf(&path) {
                Ok(()) => self.log("Driver installation requested. Confirm Windows UAC and check Device Manager for the result."),
                Err(error) => self.log(format!("Driver installation could not start: {error:#}")),
            }
        }
    }

    fn setup_panel(&mut self, ui: &mut egui::Ui) {
        Self::heading(
            ui,
            "Setup & drivers",
            "Configure Android tools and Windows USB connectivity.",
        );

        Self::card(ui, |ui| {
            Self::section(
                ui,
                "1. Android Platform Tools",
                "ADB is required to communicate with your device.",
            );
            ui.label(egui::RichText::new("Current ADB executable").color(MUTED));
            ui.add_enabled(
                !self.busy,
                egui::TextEdit::singleline(&mut self.adb_path)
                    .hint_text("adb or C:\\...\\adb.exe")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                self.link_button(ui, "Download official ZIP", setup::PLATFORM_TOOLS_URL);
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Install downloaded ZIP"))
                    .clicked()
                {
                    self.import_tools();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Choose adb.exe"))
                    .clicked()
                {
                    if let Some(file) = rfd::FileDialog::new()
                        .add_filter("Android Debug Bridge", &["exe"])
                        .pick_file()
                    {
                        self.adb_path = file.to_string_lossy().to_string();
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Download from Google's license page, then use Install downloaded ZIP. The app extracts Platform Tools to your local app-data folder automatically.").color(MUTED).size(12.0));
        });

        Self::card(ui, |ui| {
            Self::section(
                ui,
                "2. Windows USB driver",
                "Driver packages depend on the manufacturer and device model.",
            );
            ui.label(egui::RichText::new("Google USB Driver is intended for Google devices. For Xiaomi, POCO, Samsung and others, select the appropriate OEM driver.").color(MUTED));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                self.link_button(ui, "Google USB driver", setup::GOOGLE_DRIVER_URL);
                self.link_button(ui, "OEM driver directory", setup::OEM_DRIVERS_URL);
                if ui.button("Install selected INF").clicked() {
                    self.install_inf();
                }
                if ui.button("Device Manager").clicked() {
                    if let Err(error) = setup::open_device_manager() {
                        self.log(format!("Unable to open Device Manager: {error:#}"));
                    }
                }
            });
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Choose an INF downloaded from your OEM. Windows will request administrator consent and validate the driver signature. Opening the installer does not guarantee a device match.").color(WARNING).size(12.0));
        });

        Self::card(ui, |ui| {
            Self::section(
                ui,
                "3. Microsoft Visual C++",
                "Optional compatibility component for other Windows apps.",
            );
            ui.label(egui::RichText::new("Android Backup release builds statically link the C runtime and should not require installing VCRUNTIME140.dll.").color(MUTED));
            self.link_button(
                ui,
                "Download official VC++ x64 runtime",
                setup::VC_RUNTIME_URL,
            );
            ui.label(egui::RichText::new("Run the Microsoft installer yourself if needed. No runtime is installed automatically.").color(MUTED).size(12.0));
        });
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let ready = !self.busy
                && self
                    .device
                    .as_ref()
                    .is_some_and(|device| device.root.is_some());
            if ui
                .add_enabled(
                    ready,
                    egui::Button::new(egui::RichText::new("Start backup").color(TEXT).strong())
                        .fill(ACCENT_DARK)
                        .corner_radius(egui::CornerRadius::same(9)),
                )
                .clicked()
            {
                self.begin_backup();
            }
            if self.backing_up && ui.button("Cancel").clicked() {
                self.cancel.store(true, Ordering::Relaxed);
                self.log("Cancellation requested. Finishing the current I/O operation...");
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Verify backup"))
                .clicked()
            {
                self.verify_existing();
            }
            if self.busy {
                ui.spinner();
            }
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(if self.backing_up {
                    "Backup in progress"
                } else if ready {
                    "Ready to back up"
                } else {
                    "Rooted device required"
                })
                .color(MUTED),
            );
        });
    }
}

impl eframe::App for BackupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.collect_events();
        egui::TopBottomPanel::top("branded_header")
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(24, 14)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add(egui::Image::new((self.logo.id(), egui::vec2(48.0, 48.0))));
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Android Backup")
                                .color(TEXT)
                                .size(22.0)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new("ROOT-AWARE DESKTOP BACKUP")
                                .color(MUTED)
                                .size(11.0),
                        );
                    });
                    ui.add_space(24.0);
                    Self::pill(ui, "LOCAL ONLY", true);
                });
            });

        egui::TopBottomPanel::bottom("command_bar")
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(24, 12)),
            )
            .show(ctx, |ui| self.footer(ui));

        egui::SidePanel::left("navigation")
            .exact_width(218.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| self.navigation(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(28, 22)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.page {
                        Page::Overview => self.overview_panel(ui),
                        Page::Applications => self.applications_panel(ui),
                        Page::Media => self.media_panel(ui),
                        Page::Setup => self.setup_panel(ui),
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
