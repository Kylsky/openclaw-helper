mod commands;
mod openclaw;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::TaskState::default())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            commands::check_openclaw,
            commands::get_gateway_status,
            commands::run_openclaw,
            commands::open_dashboard,
            commands::open_wizard,
            commands::open_external,
            commands::uninstall_openclaw,
            commands::start_install,
            commands::cancel_task,
            commands::exec_openclaw_collect,
            commands::load_config_center_data,
            commands::load_workspace_markdowns,
            commands::save_workspace_markdown
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
