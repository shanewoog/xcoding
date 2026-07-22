#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use xcoding_core::CoreService;
use xcoding_protocol::PingResult;

#[tauri::command]
fn ping() -> Result<PingResult, String> {
    let core = CoreService::in_memory().map_err(|error| error.to_string())?;
    Ok(core.ping())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("failed to run XCoding Desktop");
}
