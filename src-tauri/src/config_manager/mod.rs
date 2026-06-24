use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use toml_edit::{value, Array, DocumentMut, Item, Table};

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("home directory not found")]
    HomeNotFound,
    #[error("{0}")]
    Validation(String),
    #[error("io error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("json error for {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("toml error for {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("sqlite error for {path}: {source}")]
    Sqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
}

type Result<T> = std::result::Result<T, ConfigError>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientId {
    Antigravity,
    Codex,
    Claude,
    Vscode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CcSwitchAgent {
    Codex,
    Claude,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    pub client: ClientId,
    pub label: &'static str,
    pub path: String,
    pub exists: bool,
    pub readable: bool,
    pub writable: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    pub name: String,
    pub config: McpServerConfig,
    pub target_clients: Vec<ClientId>,
    pub cc_switch_targets: Vec<CcSwitchAgent>,
    pub deployed_clients: BTreeMap<ClientId, McpServerConfig>,
    pub conflict: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawMcpConfig {
    pub id: String,
    pub label: String,
    pub path: String,
    pub content: String,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeckState {
    version: u32,
    #[serde(default)]
    bindings: BTreeMap<String, ServerBinding>,
}

impl Default for DeckState {
    fn default() -> Self {
        Self {
            version: 1,
            bindings: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerBinding {
    #[serde(default = "all_client_ids")]
    targets: Vec<ClientId>,
    #[serde(default)]
    cc_switch_targets: Vec<CcSwitchAgent>,
    #[serde(default)]
    last_synced_at: Option<String>,
}

#[derive(Clone, Copy)]
enum ConfigKind {
    Json { root_key: &'static str },
    CodexToml,
}

#[derive(Clone, Copy)]
struct ClientSpec {
    id: ClientId,
    label: &'static str,
    relative_path: &'static [&'static str],
    kind: ConfigKind,
}

const CLIENTS: &[ClientSpec] = &[
    ClientSpec {
        id: ClientId::Antigravity,
        label: "Antigravity",
        relative_path: &[".gemini", "antigravity", "mcp_config.json"],
        kind: ConfigKind::Json {
            root_key: "mcpServers",
        },
    },
    ClientSpec {
        id: ClientId::Codex,
        label: "Codex",
        relative_path: &[".codex", "config.toml"],
        kind: ConfigKind::CodexToml,
    },
    ClientSpec {
        id: ClientId::Claude,
        label: "Claude Code",
        relative_path: &[".claude.json"],
        kind: ConfigKind::Json {
            root_key: "mcpServers",
        },
    },
    ClientSpec {
        id: ClientId::Vscode,
        label: "VS Code",
        relative_path: &["Library", "Application Support", "Code", "User", "mcp.json"],
        kind: ConfigKind::Json {
            root_key: "servers",
        },
    },
];

pub fn get_clients_status() -> Result<Vec<ClientStatus>> {
    CLIENTS
        .iter()
        .map(|spec| {
            let path = resolve_path(spec.relative_path)?;
            let exists = path.exists();
            let readable = exists && fs::File::open(&path).is_ok();
            let writable = if exists {
                fs::OpenOptions::new().write(true).open(&path).is_ok()
            } else {
                path.parent()
                    .map(|parent| {
                        parent.exists()
                            && fs::metadata(parent)
                                .map(|m| !m.permissions().readonly())
                                .unwrap_or(false)
                    })
                    .unwrap_or(false)
            };
            let error = if exists && readable {
                read_client_servers(spec.kind, &path)
                    .err()
                    .map(|error| error.to_string())
            } else {
                None
            };
            Ok(ClientStatus {
                client: spec.id,
                label: spec.label,
                path: path.display().to_string(),
                exists,
                readable,
                writable,
                error,
            })
        })
        .collect()
}

pub fn get_mcp_servers() -> Result<Vec<ServerEntry>> {
    let source = read_master_servers()?;
    let state = read_deck_state()?;
    let mut deployed_by_client: BTreeMap<ClientId, BTreeMap<String, McpServerConfig>> =
        BTreeMap::new();

    for spec in CLIENTS {
        let path = resolve_path(spec.relative_path)?;
        if let Ok(servers) = read_client_servers(spec.kind, &path) {
            deployed_by_client.insert(spec.id, servers);
        }
    }

    Ok(source
        .into_iter()
        .map(|(name, config)| {
            let target_clients = state
                .bindings
                .get(&name)
                .map(|binding| binding.targets.clone())
                .filter(|targets| !targets.is_empty())
                .unwrap_or_else(all_client_ids);
            let cc_switch_targets = state
                .bindings
                .get(&name)
                .map(|binding| binding.cc_switch_targets.clone())
                .unwrap_or_default();
            let deployed_clients = deployed_by_client
                .iter()
                .filter_map(|(client, servers)| {
                    servers
                        .get(&name)
                        .map(|deployed_config| (*client, deployed_config.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            let conflict = target_clients
                .iter()
                .any(|client| deployed_clients.get(client) != Some(&config));
            ServerEntry {
                name,
                config,
                target_clients,
                cc_switch_targets,
                deployed_clients,
                conflict,
            }
        })
        .collect())
}

pub fn get_raw_mcp_configs() -> Result<Vec<RawMcpConfig>> {
    let mut configs = vec![raw_json_mcp_config(
        "source",
        "Source",
        master_config_path()?,
        "mcpServers",
    )];

    for spec in CLIENTS {
        let path = resolve_path(spec.relative_path)?;
        let config = match spec.kind {
            ConfigKind::Json { root_key } => raw_json_mcp_config(
                &format!("{:?}", spec.id).to_lowercase(),
                spec.label,
                path,
                root_key,
            ),
            ConfigKind::CodexToml => raw_codex_mcp_config("codex", spec.label, path),
        };
        configs.push(config);
    }

    Ok(configs)
}

pub fn save_mcp_server(
    name: &str,
    config: &McpServerConfig,
    target_clients: &[ClientId],
    cc_switch_targets: &[CcSwitchAgent],
) -> Result<()> {
    validate_name(name)?;
    validate_config(config)?;
    validate_targets(target_clients)?;

    let previous_targets = read_deck_state()?
        .bindings
        .get(name)
        .map(|binding| binding.targets.clone())
        .filter(|targets| !targets.is_empty())
        .unwrap_or_else(all_client_ids);
    let removed_targets = previous_targets
        .into_iter()
        .filter(|client| !target_clients.contains(client))
        .collect::<Vec<_>>();

    write_master_server(name, Some(config))?;
    update_server_binding(name, target_clients, cc_switch_targets, None)?;
    for client in removed_targets {
        write_client_server(client, name, None)?;
    }

    Ok(())
}

pub fn remove_mcp_server(name: &str, target_clients: &[ClientId]) -> Result<()> {
    validate_name(name)?;
    write_master_server(name, None)?;
    remove_server_binding(name)?;

    for client in target_clients {
        write_client_server(*client, name, None)?;
    }

    Ok(())
}

pub fn sync_mcp_servers() -> Result<String> {
    let source = read_master_servers()?;
    if source.is_empty() {
        return Ok(format!(
            "No MCP servers found in {}",
            master_config_path()?.display()
        ));
    }

    let mut state = read_deck_state()?;
    let mut writes = 0usize;
    let synced_at = current_timestamp();

    for (name, config) in &source {
        validate_name(name)?;
        validate_config(config)?;
        let targets = state
            .bindings
            .get(name)
            .map(|binding| binding.targets.clone())
            .filter(|targets| !targets.is_empty())
            .unwrap_or_else(all_client_ids);
        validate_targets(&targets)?;

        for client in &targets {
            write_client_server(*client, name, Some(config))?;
            writes += 1;
        }

        let cc_switch_targets = state
            .bindings
            .get(name)
            .map(|binding| binding.cc_switch_targets.clone())
            .unwrap_or_default();
        state.bindings.insert(
            name.clone(),
            ServerBinding {
                targets,
                cc_switch_targets,
                last_synced_at: Some(synced_at.clone()),
            },
        );
    }

    write_deck_state(&state)?;

    Ok(format!(
        "Synced {} MCP server(s) to {writes} client binding(s).",
        source.len()
    ))
}

pub fn sync_mcp_server(name: &str) -> Result<String> {
    validate_name(name)?;
    let source = read_master_servers()?;
    let config = source.get(name).ok_or_else(|| {
        ConfigError::Validation(format!(
            "MCP server not found in {}: {name}",
            master_config_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "~/.config/mcp/mcp.json".to_string())
        ))
    })?;
    validate_config(config)?;

    let mut state = read_deck_state()?;
    let targets = state
        .bindings
        .get(name)
        .map(|binding| binding.targets.clone())
        .filter(|targets| !targets.is_empty())
        .unwrap_or_else(all_client_ids);
    validate_targets(&targets)?;

    let mut writes = 0usize;
    for client in &targets {
        write_client_server(*client, name, Some(config))?;
        writes += 1;
    }

    let cc_switch_targets = state
        .bindings
        .get(name)
        .map(|binding| binding.cc_switch_targets.clone())
        .unwrap_or_default();
    state.bindings.insert(
        name.to_string(),
        ServerBinding {
            targets,
            cc_switch_targets,
            last_synced_at: Some(current_timestamp()),
        },
    );
    write_deck_state(&state)?;

    Ok(format!("Saved and synced {name} to {writes} client(s)."))
}

pub fn sync_codex_cc_switch() -> Result<String> {
    sync_cc_switch_agents(&[CcSwitchAgent::Codex])
}

pub fn sync_cc_switch_agents(agents: &[CcSwitchAgent]) -> Result<String> {
    if agents.is_empty() {
        return Err(ConfigError::Validation(
            "select at least one cc-switch target".to_string(),
        ));
    }
    let db_path = resolve_path(&[".cc-switch", "cc-switch.db"])?;
    if !db_path.exists() {
        return Ok(format!(
            "cc-switch database not found: {}",
            db_path.display()
        ));
    }

    backup_file(&db_path)?;
    let mut conn = Connection::open(&db_path).map_err(|source| ConfigError::Sqlite {
        path: db_path.clone(),
        source,
    })?;
    let tx = conn.transaction().map_err(|source| ConfigError::Sqlite {
        path: db_path.clone(),
        source,
    })?;

    let mut messages = Vec::new();
    for agent in agents {
        let result = match agent {
            CcSwitchAgent::Codex => sync_codex_to_cc_switch(&tx, &db_path)?,
            CcSwitchAgent::Claude => sync_claude_to_cc_switch(&tx, &db_path)?,
        };
        messages.push(result);
    }

    tx.commit().map_err(|source| ConfigError::Sqlite {
        path: db_path,
        source,
    })?;

    Ok(format!("cc-switch sync complete: {}", messages.join("; ")))
}

fn sync_codex_to_cc_switch(conn: &Connection, db_path: &Path) -> Result<String> {
    let codex_path = resolve_path(&[".codex", "config.toml"])?;

    if !codex_path.exists() {
        return Err(ConfigError::Validation(format!(
            "Codex config not found: {}",
            codex_path.display()
        )));
    }
    let codex_config = fs::read_to_string(&codex_path).map_err(|source| ConfigError::Io {
        path: codex_path.clone(),
        source,
    })?;

    let mut updates = 0usize;
    upsert_setting(conn, db_path, "common_config_codex", &codex_config)?;
    updates += 1;
    updates += update_provider_json_config_field(conn, db_path, "codex", "config", &codex_config)?;

    Ok(format!("Codex updated {updates} cc-switch row(s)"))
}

fn sync_claude_to_cc_switch(conn: &Connection, db_path: &Path) -> Result<String> {
    let claude_path = resolve_path(&[".claude.json"])?;
    if !claude_path.exists() {
        return Err(ConfigError::Validation(format!(
            "Claude Code config not found: {}",
            claude_path.display()
        )));
    }

    let claude_text = fs::read_to_string(&claude_path).map_err(|source| ConfigError::Io {
        path: claude_path.clone(),
        source,
    })?;
    let claude_value = parse_json_value(&claude_path, &claude_text)?;
    let mcp_servers = claude_value
        .get("mcpServers")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    let common_text = get_setting(conn, db_path, "common_config_claude")?;
    let mut common_config = common_text
        .as_deref()
        .map(|text| parse_json_text(db_path, text))
        .transpose()?
        .unwrap_or_else(|| Value::Object(Map::new()));
    ensure_json_object(&mut common_config);
    common_config
        .as_object_mut()
        .expect("object checked above")
        .insert("mcpServers".to_string(), mcp_servers.clone());
    let common_output =
        serde_json::to_string_pretty(&common_config).map_err(|source| ConfigError::Json {
            path: db_path.to_path_buf(),
            source,
        })?;

    let mut updates = 0usize;
    upsert_setting(conn, db_path, "common_config_claude", &common_output)?;
    updates += 1;
    updates +=
        update_provider_json_value_field(conn, db_path, "claude", "mcpServers", &mcp_servers)?;

    Ok(format!("Claude Code updated {updates} cc-switch row(s)"))
}

fn get_setting(conn: &Connection, db_path: &Path, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .map_err(|source| ConfigError::Sqlite {
        path: db_path.to_path_buf(),
        source,
    })
}

fn upsert_setting(conn: &Connection, db_path: &Path, key: &str, value_text: &str) -> Result<()> {
    let changed = conn
        .execute(
            "UPDATE settings SET value = ?1 WHERE key = ?2",
            params![value_text, key],
        )
        .map_err(|source| ConfigError::Sqlite {
            path: db_path.to_path_buf(),
            source,
        })?;
    if changed == 0 {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value_text],
        )
        .map_err(|source| ConfigError::Sqlite {
            path: db_path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn update_provider_json_config_field(
    conn: &Connection,
    db_path: &Path,
    app_type: &str,
    field: &str,
    value_text: &str,
) -> Result<usize> {
    update_provider_json_value_field(
        conn,
        db_path,
        app_type,
        field,
        &Value::String(value_text.to_string()),
    )
}

fn update_provider_json_value_field(
    conn: &Connection,
    db_path: &Path,
    app_type: &str,
    field: &str,
    value: &Value,
) -> Result<usize> {
    let rows = current_provider_rows(conn, db_path, app_type)?;
    let mut updates = 0usize;
    for (id, settings_config) in rows {
        let mut settings_value = parse_json_text(db_path, &settings_config)?;
        ensure_json_object(&mut settings_value);
        settings_value
            .as_object_mut()
            .expect("object checked above")
            .insert(field.to_string(), value.clone());
        let output =
            serde_json::to_string(&settings_value).map_err(|source| ConfigError::Json {
                path: db_path.to_path_buf(),
                source,
            })?;
        updates += conn
            .execute(
                "UPDATE providers SET settings_config = ?1 WHERE id = ?2",
                params![output, id],
            )
            .map_err(|source| ConfigError::Sqlite {
                path: db_path.to_path_buf(),
                source,
            })?;
    }
    Ok(updates)
}

fn current_provider_rows(
    conn: &Connection,
    db_path: &Path,
    app_type: &str,
) -> Result<Vec<(String, String)>> {
    let has_current = table_has_column(conn, "providers", "is_current")?;
    let sql = if has_current {
        "SELECT id, settings_config FROM providers WHERE app_type = ?1 AND is_current = 1"
    } else {
        "SELECT id, settings_config FROM providers WHERE app_type = ?1"
    };
    let mut statement = conn.prepare(sql).map_err(|source| ConfigError::Sqlite {
        path: db_path.to_path_buf(),
        source,
    })?;
    let rows = statement
        .query_map(params![app_type], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|source| ConfigError::Sqlite {
            path: db_path.to_path_buf(),
            source,
        })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|source| ConfigError::Sqlite {
            path: db_path.to_path_buf(),
            source,
        })
}

fn parse_json_text(path: &Path, text: &str) -> Result<Value> {
    parse_json_value(path, text)
}

fn ensure_json_object(value: &mut Value) {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
}

fn read_client_servers(kind: ConfigKind, path: &Path) -> Result<BTreeMap<String, McpServerConfig>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    match kind {
        ConfigKind::Json { root_key } => read_json_servers(path, root_key),
        ConfigKind::CodexToml => read_codex_servers(path),
    }
}

fn read_master_servers() -> Result<BTreeMap<String, McpServerConfig>> {
    read_json_servers_if_exists(&master_config_path()?, "mcpServers")
}

fn write_master_server(name: &str, config: Option<&McpServerConfig>) -> Result<()> {
    write_json_server(&master_config_path()?, "mcpServers", name, config)
}

fn write_client_server(
    client: ClientId,
    name: &str,
    config: Option<&McpServerConfig>,
) -> Result<()> {
    let spec = spec_for(client)?;
    let path = resolve_path(spec.relative_path)?;
    match spec.kind {
        ConfigKind::Json { root_key } => write_json_server(&path, root_key, name, config),
        ConfigKind::CodexToml => write_codex_server(&path, name, config),
    }
}

fn raw_json_mcp_config(id: &str, label: &str, path: PathBuf, root_key: &str) -> RawMcpConfig {
    match read_raw_json_mcp_config(&path, root_key) {
        Ok(content) => RawMcpConfig {
            id: id.to_string(),
            label: label.to_string(),
            path: path.display().to_string(),
            content,
            error: None,
        },
        Err(error) => RawMcpConfig {
            id: id.to_string(),
            label: label.to_string(),
            path: path.display().to_string(),
            content: String::new(),
            error: Some(error.to_string()),
        },
    }
}

fn raw_codex_mcp_config(id: &str, label: &str, path: PathBuf) -> RawMcpConfig {
    match read_raw_codex_mcp_config(&path) {
        Ok(content) => RawMcpConfig {
            id: id.to_string(),
            label: label.to_string(),
            path: path.display().to_string(),
            content,
            error: None,
        },
        Err(error) => RawMcpConfig {
            id: id.to_string(),
            label: label.to_string(),
            path: path.display().to_string(),
            content: String::new(),
            error: Some(error.to_string()),
        },
    }
}

fn read_raw_json_mcp_config(path: &Path, root_key: &str) -> Result<String> {
    if !path.exists() {
        return Ok(format!("{{\n  \"{root_key}\": {{}}\n}}"));
    }

    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let value = parse_json_value(path, &text)?;
    let mcp_value = value
        .get(root_key)
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mut output = Map::new();
    output.insert(root_key.to_string(), mcp_value);
    serde_json::to_string_pretty(&output).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn read_raw_codex_mcp_config(path: &Path) -> Result<String> {
    if !path.exists() {
        return Ok("[mcp_servers]\n".to_string());
    }

    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let document = text
        .parse::<DocumentMut>()
        .map_err(|source| ConfigError::Toml {
            path: path.to_path_buf(),
            source,
        })?;
    let Some(table) = document.get("mcp_servers").and_then(Item::as_table) else {
        return Ok("[mcp_servers]\n".to_string());
    };

    let mut output = DocumentMut::new();
    output["mcp_servers"] = Item::Table(table.clone());
    Ok(output.to_string())
}

fn read_json_servers_if_exists(
    path: &Path,
    root_key: &str,
) -> Result<BTreeMap<String, McpServerConfig>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    read_json_servers(path, root_key)
}

fn read_deck_state() -> Result<DeckState> {
    let path = deck_state_path()?;
    if !path.exists() {
        return Ok(DeckState::default());
    }
    let text = fs::read_to_string(&path).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| ConfigError::Json { path, source })
}

fn write_deck_state(state: &DeckState) -> Result<()> {
    let path = deck_state_path()?;
    let text = serde_json::to_string_pretty(state).map_err(|source| ConfigError::Json {
        path: path.clone(),
        source,
    })?;
    atomic_write(&path, &format!("{text}\n"))
}

fn update_server_binding(
    name: &str,
    target_clients: &[ClientId],
    cc_switch_targets: &[CcSwitchAgent],
    last_synced_at: Option<String>,
) -> Result<()> {
    let mut state = read_deck_state()?;
    state.bindings.insert(
        name.to_string(),
        ServerBinding {
            targets: target_clients.to_vec(),
            cc_switch_targets: cc_switch_targets.to_vec(),
            last_synced_at,
        },
    );
    write_deck_state(&state)
}

fn remove_server_binding(name: &str) -> Result<()> {
    let mut state = read_deck_state()?;
    state.bindings.remove(name);
    write_deck_state(&state)
}

fn read_json_servers(path: &Path, root_key: &str) -> Result<BTreeMap<String, McpServerConfig>> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let value = parse_json_value(path, &text)?;
    let Some(object) = value.get(root_key).and_then(Value::as_object) else {
        return Ok(BTreeMap::new());
    };

    let mut servers = BTreeMap::new();
    for (name, server_value) in object {
        if let Some(config) = config_from_json(server_value) {
            servers.insert(name.clone(), config);
        }
    }
    Ok(servers)
}

fn read_codex_servers(path: &Path) -> Result<BTreeMap<String, McpServerConfig>> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let document = text
        .parse::<DocumentMut>()
        .map_err(|source| ConfigError::Toml {
            path: path.to_path_buf(),
            source,
        })?;
    let mut servers = BTreeMap::new();
    let Some(table) = document.get("mcp_servers").and_then(Item::as_table) else {
        return Ok(servers);
    };

    for (name, item) in table.iter() {
        if let Some(server_table) = item.as_table() {
            let command = server_table
                .get("command")
                .and_then(Item::as_str)
                .unwrap_or_default()
                .to_string();
            if command.is_empty() {
                continue;
            }
            let args = server_table
                .get("args")
                .and_then(Item::as_array)
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let env = server_table
                .get("env")
                .and_then(Item::as_table_like)
                .map(|table| {
                    table
                        .iter()
                        .filter_map(|(key, value)| {
                            value
                                .as_str()
                                .map(|value| (key.to_string(), value.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            servers.insert(name.to_string(), McpServerConfig { command, args, env });
        }
    }

    Ok(servers)
}

fn write_json_server(
    path: &Path,
    root_key: &str,
    name: &str,
    config: Option<&McpServerConfig>,
) -> Result<()> {
    let mut value = if path.exists() {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        parse_json_value(path, &text)?
    } else {
        json!({})
    };

    if !value.is_object() {
        value = json!({});
    }
    let root = value.as_object_mut().expect("object checked above");
    let servers = root
        .entry(root_key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !servers.is_object() {
        *servers = Value::Object(Map::new());
    }
    let servers_object = servers.as_object_mut().expect("object checked above");

    match config {
        Some(config) => {
            let mut server_value = servers_object
                .get(name)
                .cloned()
                .filter(Value::is_object)
                .unwrap_or_else(|| Value::Object(Map::new()));
            upsert_json_config(&mut server_value, config);
            servers_object.insert(name.to_string(), server_value);
        }
        None => {
            servers_object.remove(name);
        }
    }

    let text = serde_json::to_string_pretty(&value).map_err(|source| ConfigError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    atomic_write(path, &format!("{text}\n"))
}

fn write_codex_server(path: &Path, name: &str, config: Option<&McpServerConfig>) -> Result<()> {
    let text = if path.exists() {
        fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?
    } else {
        String::new()
    };
    let mut document = text
        .parse::<DocumentMut>()
        .map_err(|source| ConfigError::Toml {
            path: path.to_path_buf(),
            source,
        })?;

    if !document.as_table().contains_key("mcp_servers") {
        document["mcp_servers"] = Item::Table(Table::new());
    }

    let servers = document["mcp_servers"]
        .as_table_mut()
        .ok_or_else(|| ConfigError::Validation("[mcp_servers] is not a TOML table".to_string()))?;

    match config {
        Some(config) => {
            let mut server_table = servers
                .get(name)
                .and_then(Item::as_table)
                .cloned()
                .unwrap_or_else(Table::new);

            server_table["command"] = value(config.command.clone());
            if config.args.is_empty() {
                server_table.remove("args");
            } else {
                let mut args = Array::default();
                for arg in &config.args {
                    args.push(arg.as_str());
                }
                server_table["args"] = value(args);
            }
            if config.env.is_empty() {
                server_table.remove("env");
            } else {
                let mut env = Table::new();
                for (key, value_text) in &config.env {
                    env[key] = value(value_text.clone());
                }
                server_table["env"] = Item::Table(env);
            }
            servers[name] = Item::Table(server_table);
        }
        None => {
            servers.remove(name);
        }
    }

    atomic_write(path, &document.to_string())
}

fn upsert_json_config(value: &mut Value, config: &McpServerConfig) {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    let object = value.as_object_mut().expect("object checked above");
    object.insert("command".to_string(), Value::String(config.command.clone()));
    object.insert(
        "args".to_string(),
        Value::Array(config.args.iter().cloned().map(Value::String).collect()),
    );
    object.insert(
        "env".to_string(),
        Value::Object(
            config
                .env
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect(),
        ),
    );
}

fn config_from_json(value: &Value) -> Option<McpServerConfig> {
    let object = value.as_object()?;
    let command = object.get("command")?.as_str()?.to_string();
    let args = object
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let env = object
        .get("env")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    Some(McpServerConfig { command, args, env })
}

fn parse_json_value(path: &Path, text: &str) -> Result<Value> {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => Ok(value),
        Err(strict_error) => {
            let relaxed = strip_json_comments_and_trailing_commas(text);
            serde_json::from_str::<Value>(&relaxed).map_err(|_| ConfigError::Json {
                path: path.to_path_buf(),
                source: strict_error,
            })
        }
    }
}

fn strip_json_comments_and_trailing_commas(text: &str) -> String {
    let without_comments = strip_json_comments(text);
    strip_json_trailing_commas(&without_comments)
}

fn strip_json_comments(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }

        if ch == '/' {
            match chars.peek().copied() {
                Some('/') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            output.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if next == '\n' {
                            output.push('\n');
                        }
                        if previous == '*' && next == '/' {
                            break;
                        }
                        previous = next;
                    }
                }
                _ => output.push(ch),
            }
            continue;
        }

        output.push(ch);
    }

    output
}

fn strip_json_trailing_commas(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    while index < chars.len() {
        let ch = chars[index];
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if ch == '"' {
            in_string = true;
            output.push(ch);
            index += 1;
            continue;
        }

        if ch == ',' {
            let mut lookahead = index + 1;
            while lookahead < chars.len() && chars[lookahead].is_whitespace() {
                lookahead += 1;
            }
            if lookahead < chars.len() && matches!(chars[lookahead], '}' | ']') {
                index += 1;
                continue;
            }
        }

        output.push(ch);
        index += 1;
    }

    output
}

fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    if path.exists() {
        backup_file(path)?;
    }

    let temp_path = path.with_extension(format!(
        "{}tmp",
        path.extension().and_then(|ext| ext.to_str()).unwrap_or("")
    ));
    {
        let mut file = fs::File::create(&temp_path).map_err(|source| ConfigError::Io {
            path: temp_path.clone(),
            source,
        })?;
        file.write_all(contents.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|source| ConfigError::Io {
                path: temp_path.clone(),
                source,
            })?;
    }
    fs::rename(&temp_path, path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}

fn backup_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let backup_path = path.with_extension(format!(
        "{}mcp-deck.bak",
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!("{extension}."))
            .unwrap_or_default()
    ));
    fs::copy(path, &backup_path).map_err(|source| ConfigError::Io {
        path: backup_path,
        source,
    })?;
    Ok(())
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let sql = format!("PRAGMA table_info({table})");
    let mut statement = conn.prepare(&sql).map_err(|source| ConfigError::Sqlite {
        path: resolve_path(&[".cc-switch", "cc-switch.db"]).unwrap_or_default(),
        source,
    })?;
    let mut rows = statement.query([]).map_err(|source| ConfigError::Sqlite {
        path: resolve_path(&[".cc-switch", "cc-switch.db"]).unwrap_or_default(),
        source,
    })?;
    while let Some(row) = rows.next().map_err(|source| ConfigError::Sqlite {
        path: resolve_path(&[".cc-switch", "cc-switch.db"]).unwrap_or_default(),
        source,
    })? {
        let name: String = row.get(1).map_err(|source| ConfigError::Sqlite {
            path: resolve_path(&[".cc-switch", "cc-switch.db"]).unwrap_or_default(),
            source,
        })?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(ConfigError::Validation(
            "server name cannot be empty".to_string(),
        ));
    }
    if name.contains('\n') || name.contains('\r') {
        return Err(ConfigError::Validation(
            "server name cannot contain newlines".to_string(),
        ));
    }
    Ok(())
}

fn validate_config(config: &McpServerConfig) -> Result<()> {
    if config.command.trim().is_empty() {
        return Err(ConfigError::Validation(
            "command cannot be empty".to_string(),
        ));
    }
    for key in config.env.keys() {
        if key.trim().is_empty() || key.contains('=') {
            return Err(ConfigError::Validation(format!("invalid env key: {key}")));
        }
    }
    Ok(())
}

fn validate_targets(target_clients: &[ClientId]) -> Result<()> {
    if target_clients.is_empty() {
        return Err(ConfigError::Validation(
            "at least one target client is required".to_string(),
        ));
    }
    Ok(())
}

fn all_client_ids() -> Vec<ClientId> {
    CLIENTS.iter().map(|spec| spec.id).collect()
}

fn spec_for(client: ClientId) -> Result<&'static ClientSpec> {
    CLIENTS
        .iter()
        .find(|spec| spec.id == client)
        .ok_or_else(|| ConfigError::Validation(format!("unsupported client: {client:?}")))
}

fn resolve_path(parts: &[&str]) -> Result<PathBuf> {
    let mut path = dirs::home_dir().ok_or(ConfigError::HomeNotFound)?;
    for part in parts {
        path.push(part);
    }
    Ok(path)
}

fn master_config_path() -> Result<PathBuf> {
    resolve_path(&[".config", "mcp", "mcp.json"])
}

fn deck_state_path() -> Result<PathBuf> {
    resolve_path(&[".config", "mcp-deck", "state.json"])
}

fn current_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_write_preserves_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mcp.json");
        fs::write(
            &path,
            r#"{
  "servers": {
    "demo": {
      "command": "old",
      "args": [],
      "env": {},
      "timeout": 15
    }
  },
  "other": true
}"#,
        )
        .unwrap();

        write_json_server(
            &path,
            "servers",
            "demo",
            Some(&McpServerConfig {
                command: "npx".to_string(),
                args: vec!["-y".to_string()],
                env: BTreeMap::from([("TOKEN".to_string(), "secret".to_string())]),
            }),
        )
        .unwrap();

        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["other"], true);
        assert_eq!(value["servers"]["demo"]["timeout"], 15);
        assert_eq!(value["servers"]["demo"]["command"], "npx");
    }

    #[test]
    fn codex_write_keeps_existing_comments_and_tables() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            r#"# keep this
model = "gpt-5"

[mcp_servers.old]
command = "uvx"
"#,
        )
        .unwrap();

        write_codex_server(
            &path,
            "demo",
            Some(&McpServerConfig {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "pkg".to_string()],
                env: BTreeMap::new(),
            }),
        )
        .unwrap();

        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("# keep this"));
        assert!(text.contains("model = \"gpt-5\""));
        assert!(text.contains("[mcp_servers.demo]"));
        assert!(text.contains("command = \"npx\""));
    }

    #[test]
    fn json_reader_accepts_jsonc_comments() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mcp.json");
        fs::write(
            &path,
            r#"{
  "servers": {
    // disabled example
    "demo": {
      "command": "npx",
      "args": ["-y",],
      "env": {}
    },
  }
}"#,
        )
        .unwrap();

        let servers = read_json_servers(&path, "servers").unwrap();
        assert_eq!(servers["demo"].command, "npx");
        assert_eq!(servers["demo"].args, vec!["-y"]);
    }

    #[test]
    fn raw_json_view_returns_only_mcp_root() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("mcp.json");
        fs::write(
            &path,
            r#"{
  "servers": {
    "demo": {
      "command": "npx",
      "args": ["pkg",],
      "env": {}
    },
  },
  "unrelated": true
}"#,
        )
        .unwrap();

        let text = read_raw_json_mcp_config(&path, "servers").unwrap();
        assert!(text.contains("\"servers\""));
        assert!(text.contains("\"demo\""));
        assert!(!text.contains("unrelated"));
    }

    #[test]
    fn raw_codex_view_returns_only_mcp_servers_table() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            r#"model = "gpt-5"

[mcp_servers.demo]
command = "npx"
args = ["pkg"]

[profiles.default]
model = "gpt-5"
"#,
        )
        .unwrap();

        let text = read_raw_codex_mcp_config(&path).unwrap();
        assert!(text.contains("[mcp_servers.demo]"));
        assert!(text.contains("command = \"npx\""));
        assert!(!text.contains("[profiles.default]"));
    }
}
