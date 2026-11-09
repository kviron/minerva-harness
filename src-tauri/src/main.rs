#![windows_subsystem = "windows"]
mod harness;
fn main() {
    harness::run(tauri::generate_context!(), false);
}
