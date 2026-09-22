//! whisperexe masaüstü kabuğu — YALNIZCA `tauri` özelliğiyle derlenir.
//!
//! Saf mantık (`Shell`, `overlay::render`, `version`) Tauri'siz test edilir;
//! bu ikili, o mantığı F9 global-kısayolu + overlay penceresi + Tauri updater
//! ile çalıştırır. Özel indirici YOKTUR (güncelleme beslemesi broker'dandır).

#![cfg(feature = "tauri")]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri_shell::tauri_app::run();
}
