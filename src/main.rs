#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod adb;
mod backup;
mod manifest;
mod setup;
mod ui;

fn main() -> eframe::Result<()> {
    let icon = eframe::egui::IconData {
        rgba: include_bytes!("../assets/icon.rgba").to_vec(),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 780.0])
            .with_min_inner_size([860.0, 640.0])
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Android Backup",
        options,
        Box::new(|cc| Ok(Box::new(ui::BackupApp::new(cc)))),
    )
}
