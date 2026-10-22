// No console window behind the launcher in release builds on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    if let Err(error) = launcher::run() {
        eprintln!("Dauntless Reforged Revival Launcher could not start: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
