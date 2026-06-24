mod config_manager;

use config_manager::{CcSwitchAgent, ClientId, McpServerConfig};

#[tauri::command]
fn get_clients_status() -> Result<Vec<config_manager::ClientStatus>, String> {
    config_manager::get_clients_status().map_err(|error| error.to_string())
}

#[tauri::command]
fn get_mcp_servers() -> Result<Vec<config_manager::ServerEntry>, String> {
    config_manager::get_mcp_servers().map_err(|error| error.to_string())
}

#[tauri::command]
fn get_raw_mcp_configs() -> Result<Vec<config_manager::RawMcpConfig>, String> {
    config_manager::get_raw_mcp_configs().map_err(|error| error.to_string())
}

#[tauri::command]
fn save_mcp_server(
    name: String,
    config: McpServerConfig,
    target_clients: Vec<ClientId>,
    cc_switch_targets: Vec<CcSwitchAgent>,
) -> Result<(), String> {
    config_manager::save_mcp_server(&name, &config, &target_clients, &cc_switch_targets)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn remove_mcp_server(name: String, target_clients: Vec<ClientId>) -> Result<(), String> {
    config_manager::remove_mcp_server(&name, &target_clients).map_err(|error| error.to_string())
}

#[tauri::command]
fn sync_mcp_servers() -> Result<String, String> {
    config_manager::sync_mcp_servers().map_err(|error| error.to_string())
}

#[tauri::command]
fn sync_mcp_server(name: String) -> Result<String, String> {
    config_manager::sync_mcp_server(&name).map_err(|error| error.to_string())
}

#[tauri::command]
fn sync_cc_switch_agents(agents: Vec<CcSwitchAgent>) -> Result<String, String> {
    config_manager::sync_cc_switch_agents(&agents).map_err(|error| error.to_string())
}

#[tauri::command]
fn sync_codex_cc_switch() -> Result<String, String> {
    config_manager::sync_codex_cc_switch().map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_clients_status,
            get_mcp_servers,
            get_raw_mcp_configs,
            save_mcp_server,
            remove_mcp_server,
            sync_mcp_servers,
            sync_mcp_server,
            sync_cc_switch_agents,
            sync_codex_cc_switch
        ])
        .run(tauri::generate_context!())
        .expect("error while running MCP Deck");
}
