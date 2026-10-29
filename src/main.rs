mod adb;
mod backup;
mod manifest;
mod ui;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 750.0])
            .with_min_inner_size([760.0, 540.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Android Backup",
        options,
        Box::new(|_cc| Ok(Box::new(ui::BackupApp::new()))),
    )
}
