#![windows_subsystem = "windows"]
#[cfg(windows)]
#[path = "../harness.rs"]
mod harness;
fn main() {
    #[cfg(windows)]
    harness::run_fme();
    #[cfg(not(windows))]
    eprintln!("FME embedding requires Windows.");
}
