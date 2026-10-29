#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod adb;
mod backup;
mod manifest;
mod ui;

fn main() -> eframe::Result<()> {
    let icon = eframe::egui::IconData {
        rgba: include_bytes!("../assets/icon.rgba").to_vec(),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 750.0])
            .with_min_inner_size([760.0, 540.0])
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Android Backup",
        options,
        Box::new(|_cc| Ok(Box::new(ui::BackupApp::new()))),
    )
}
