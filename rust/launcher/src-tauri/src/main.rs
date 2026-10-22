// No console window behind the launcher in release builds on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    // Offline play's services run as children of the launcher (D12)
    #[cfg(feature = "offline")]
    if let Some(code) = launcher::offline::service::run_if_requested() {
        return code;
    }
    if let Err(error) = launcher::run() {
        eprintln!("Dauntless Revived Launcher could not start: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
