import { invoke } from "@tauri-apps/api/core";
import {
  Check,
  ClipboardPaste,
  DatabaseZap,
  Eye,
  EyeOff,
  FileCode,
  House,
  Plus,
  RefreshCw,
  Save,
  Server,
  Trash2,
  UploadCloud,
  X,
} from "lucide-react";
import { FormEvent, useEffect, useMemo, useState } from "react";

type ClientId = "antigravity" | "codex" | "claude" | "vscode";
type CcSwitchAgent = "codex" | "claude";

type McpServerConfig = {
  command: string;
  args: string[];
  env: Record<string, string>;
};

type ClientStatus = {
  client: ClientId;
  label: string;
  path: string;
  exists: boolean;
  readable: boolean;
  writable: boolean;
  error?: string | null;
};

type ServerEntry = {
  name: string;
  config: McpServerConfig;
  targetClients: ClientId[];
  ccSwitchTargets: CcSwitchAgent[];
  deployedClients: Partial<Record<ClientId, McpServerConfig>>;
  conflict: boolean;
};

type RawMcpConfig = {
  id: string;
  label: string;
  path: string;
  content: string;
  error?: string | null;
};

type FormState = {
  name: string;
  command: string;
  argsText: string;
  envText: string;
  targets: Record<ClientId, boolean>;
  ccSwitchTargets: Record<CcSwitchAgent, boolean>;
};

type ParsedMcpEntry = {
  name: string;
  config: McpServerConfig;
};

const clients: Array<{ id: ClientId; label: string }> = [
  { id: "antigravity", label: "Antigravity" },
  { id: "codex", label: "Codex" },
  { id: "claude", label: "Claude Code" },
  { id: "vscode", label: "VS Code" },
];

const ccSwitchAgents: Array<{ id: CcSwitchAgent; label: string }> = [
  { id: "codex", label: "Codex" },
  { id: "claude", label: "Claude Code" },
];

const emptyForm: FormState = {
  name: "",
  command: "",
  argsText: "",
  envText: "",
  targets: {
    antigravity: true,
    codex: true,
    claude: true,
    vscode: true,
  },
  ccSwitchTargets: {
    codex: false,
    claude: false,
  },
};

function formatArgs(args: string[]) {
  return args.join("\n");
}

function parseArgs(text: string) {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function formatEnv(env: Record<string, string>) {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

function parseEnv(text: string) {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .reduce<Record<string, string>>((acc, line) => {
      const index = line.indexOf("=");
      if (index > 0) {
        acc[line.slice(0, index).trim()] = line.slice(index + 1).trim();
      }
      return acc;
    }, {});
}

function isSecretKey(key: string) {
  return /(token|secret|key|password|credential|auth)/i.test(key);
}

function sameConfig(a: McpServerConfig, b: McpServerConfig) {
  return JSON.stringify(a) === JSON.stringify(b);
}

function normalizeMcpConfig(value: unknown): McpServerConfig | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }
  const record = value as Record<string, unknown>;
  if (typeof record.command !== "string") {
    return null;
  }
  const args = Array.isArray(record.args)
    ? record.args.filter((item): item is string => typeof item === "string")
    : [];
  const env =
    record.env && typeof record.env === "object" && !Array.isArray(record.env)
      ? Object.entries(record.env as Record<string, unknown>).reduce<Record<string, string>>(
          (acc, [key, envValue]) => {
            if (typeof envValue === "string") {
              acc[key] = envValue;
            }
            return acc;
          },
          {},
        )
      : {};
  return {
    command: record.command,
    args,
    env,
  };
}

function stripJsonCommentsAndTrailingCommas(text: string) {
  return text
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|\s)\/\/.*$/gm, "$1")
    .replace(/,\s*([}\]])/g, "$1");
}

function parseMcpJsonEntries(text: string, fallbackName: string): ParsedMcpEntry[] {
  const trimmed = text.trim();
  if (!trimmed) {
    throw new Error("请先粘贴 MCP JSON");
  }

  const normalized = stripJsonCommentsAndTrailingCommas(trimmed);
  const candidates = [normalized, `{${normalized}}`];
  let parsed: unknown = null;
  let lastError: unknown = null;

  for (const candidate of candidates) {
    try {
      parsed = JSON.parse(candidate);
      break;
    } catch (err) {
      lastError = err;
    }
  }

  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw lastError instanceof Error ? lastError : new Error("无法解析 MCP JSON");
  }

  const record = parsed as Record<string, unknown>;
  const root =
    record.mcpServers && typeof record.mcpServers === "object"
      ? (record.mcpServers as Record<string, unknown>)
      : record.servers && typeof record.servers === "object"
        ? (record.servers as Record<string, unknown>)
        : record;

  const directConfig = normalizeMcpConfig(root);
  if (directConfig) {
    return [{ name: fallbackName || "imported-server", config: directConfig }];
  }

  const entries = Object.entries(root)
    .map(([name, value]) => {
      const config = normalizeMcpConfig(value);
      return config ? { name, config } : null;
    })
    .filter((entry): entry is ParsedMcpEntry => Boolean(entry));

  if (entries.length === 0) {
    throw new Error("没有识别到带 command 的 MCP server 配置");
  }
  return entries;
}

function App() {
  const [servers, setServers] = useState<ServerEntry[]>([]);
  const [statuses, setStatuses] = useState<ClientStatus[]>([]);
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [form, setForm] = useState<FormState>(emptyForm);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [showSecrets, setShowSecrets] = useState(false);
  const [showNeedsSyncOnly, setShowNeedsSyncOnly] = useState(false);
  const [jsonImportOpen, setJsonImportOpen] = useState(false);
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [rawConfigsOpen, setRawConfigsOpen] = useState(false);
  const [rawConfigsLoading, setRawConfigsLoading] = useState(false);
  const [rawConfigs, setRawConfigs] = useState<RawMcpConfig[]>([]);
  const [activeRawConfigId, setActiveRawConfigId] = useState<string | null>(null);
  const [jsonInput, setJsonInput] = useState("");
  const [parsedJsonEntries, setParsedJsonEntries] = useState<ParsedMcpEntry[]>([]);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const selected = useMemo(
    () => servers.find((serverEntry) => serverEntry.name === selectedName),
    [selectedName, servers],
  );
  const activeRawConfig = useMemo(
    () =>
      rawConfigs.find((config) => config.id === activeRawConfigId) ??
      rawConfigs[0] ??
      null,
    [activeRawConfigId, rawConfigs],
  );

  async function loadData() {
    setLoading(true);
    setError(null);
    try {
      const [serverList, clientStatuses] = await Promise.all([
        invoke<ServerEntry[]>("get_mcp_servers"),
        invoke<ClientStatus[]>("get_clients_status"),
      ]);
      setServers(serverList);
      setStatuses(clientStatuses);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    void loadData();
  }, []);

  function openServer(entry: ServerEntry) {
    setCreating(false);
    setSelectedName(entry.name);
    setForm({
      name: entry.name,
      command: entry.config.command,
      argsText: formatArgs(entry.config.args),
      envText: formatEnv(entry.config.env),
      targets: clients.reduce(
        (acc, client) => {
          acc[client.id] = entry.targetClients.includes(client.id);
          return acc;
        },
        { ...emptyForm.targets },
      ),
      ccSwitchTargets: ccSwitchAgents.reduce(
        (acc, agent) => {
          acc[agent.id] = entry.ccSwitchTargets.includes(agent.id);
          return acc;
        },
        { ...emptyForm.ccSwitchTargets },
      ),
    });
    setNotice(null);
  }

  function newServer() {
    setCreating(true);
    setSelectedName(null);
    setForm(emptyForm);
    setNotice(null);
    setError(null);
  }

  function showOverview() {
    setCreating(false);
    setSelectedName(null);
    setForm(emptyForm);
    setDeleteConfirmOpen(false);
    setNotice(null);
    setError(null);
  }

  function selectedTargetClients() {
    return clients
      .filter((client) => form.targets[client.id])
      .map((client) => client.id);
  }

  function selectedCcSwitchAgents() {
    return ccSwitchAgents
      .filter((agent) => form.ccSwitchTargets[agent.id])
      .map((agent) => agent.id);
  }

  async function saveServer(event: FormEvent) {
    event.preventDefault();
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      const targets = selectedTargetClients();
      const ccSwitchAgents = selectedCcSwitchAgents();
      if (targets.length === 0) {
        throw new Error("至少选择一个目标客户端");
      }
      const savedName = form.name.trim();
      await invoke("save_mcp_server", {
        name: savedName,
        config: {
          command: form.command.trim(),
          args: parseArgs(form.argsText),
          env: parseEnv(form.envText),
        },
        targetClients: targets,
        ccSwitchTargets: ccSwitchAgents,
      });
      const result = await invoke<string>("sync_mcp_server", { name: savedName });
      const ccSwitchResult =
        ccSwitchAgents.length > 0
          ? await invoke<string>("sync_cc_switch_agents", { agents: ccSwitchAgents })
          : "";
      setNotice(ccSwitchResult ? `${result} ${ccSwitchResult}` : result);
      setCreating(false);
      setSelectedName(savedName);
      await loadData();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function removeServer() {
    const targets = selectedTargetClients();
    const ccSwitchAgents = selectedCcSwitchAgents();
    if (!form.name.trim() || targets.length === 0) return;
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      await invoke("remove_mcp_server", {
        name: form.name.trim(),
        targetClients: targets,
      });
      const ccSwitchResult =
        ccSwitchAgents.length > 0
          ? await invoke<string>("sync_cc_switch_agents", { agents: ccSwitchAgents })
          : "";
      setNotice(
        ccSwitchResult
          ? `已从主配置和选定客户端配置中删除该 MCP 服务。${ccSwitchResult}`
          : "已从主配置和选定客户端配置中删除该 MCP 服务",
      );
      setDeleteConfirmOpen(false);
      setSelectedName(null);
      setForm(emptyForm);
      await loadData();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function syncAllClients() {
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      const result = await invoke<string>("sync_mcp_servers");
      setNotice(result);
      await loadData();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  async function openRawConfigs() {
    setRawConfigsOpen(true);
    setRawConfigsLoading(true);
    setError(null);
    setNotice(null);
    try {
      const configs = await invoke<RawMcpConfig[]>("get_raw_mcp_configs");
      setRawConfigs(configs);
      setActiveRawConfigId((current) => current ?? configs[0]?.id ?? null);
    } catch (err) {
      setRawConfigs([]);
      setActiveRawConfigId(null);
      setError(String(err));
    } finally {
      setRawConfigsLoading(false);
    }
  }

  function closeRawConfigs() {
    setRawConfigsOpen(false);
  }

  function openJsonImport() {
    setJsonImportOpen(true);
    setError(null);
    setNotice(null);
  }

  function closeJsonImport() {
    setJsonImportOpen(false);
    setParsedJsonEntries([]);
  }

  function parseJsonImport() {
    setError(null);
    setNotice(null);
    try {
      const entries = parseMcpJsonEntries(jsonInput, form.name || selectedName || "");
      setParsedJsonEntries(entries);
      setNotice(`识别到 ${entries.length} 个 MCP server`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  function useImportedEntry(entry: ParsedMcpEntry) {
    setCreating(true);
    setSelectedName(null);
    setForm({
      ...form,
      name: entry.name,
      command: entry.config.command,
      argsText: formatArgs(entry.config.args),
      envText: formatEnv(entry.config.env),
    });
    closeJsonImport();
    setNotice(`已填入 ${entry.name}`);
  }

  async function importAllEntries() {
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      const entries =
        parsedJsonEntries.length > 0
          ? parsedJsonEntries
          : parseMcpJsonEntries(jsonInput, form.name || selectedName || "");
      const targetClients = clients.map((client) => client.id);
      for (const entry of entries) {
        await invoke("save_mcp_server", {
          name: entry.name,
          config: entry.config,
          targetClients,
          ccSwitchTargets: [],
        });
      }
      closeJsonImport();
      setCreating(false);
      setSelectedName(null);
      setNotice(`已导入 ${entries.length} 个 MCP 到 ~/.config/mcp/mcp.json`);
      await loadData();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  const totalEnabled = servers.reduce(
    (sum, entry) => sum + entry.targetClients.length,
    0,
  );
  const conflicts = servers.filter((entry) => entry.conflict).length;
  const visibleServers = showNeedsSyncOnly
    ? servers.filter((entry) => entry.conflict)
    : servers;
  const editing = creating || Boolean(selected);
  const deleteTargets = clients.filter((client) => form.targets[client.id]);
  const deleteCcSwitchTargets = ccSwitchAgents.filter(
    (agent) => form.ccSwitchTargets[agent.id],
  );

  return (
    <main className="app-shell">
      <section className="sidebar">
        <button className="brand-row" onClick={showOverview} type="button">
          <div className="brand-mark">
            <Server size={20} />
          </div>
          <div>
            <h1>MCP Deck</h1>
            <span>Desktop config console</span>
          </div>
        </button>

        <div className="metric-grid">
          <button
            className={!showNeedsSyncOnly ? "metric-card active" : "metric-card"}
            onClick={() => setShowNeedsSyncOnly(false)}
            type="button"
          >
            <strong>{servers.length}</strong>
            <span>Servers</span>
          </button>
          <div className="metric-card">
            <strong>{totalEnabled}</strong>
            <span>Bindings</span>
          </div>
          <button
            className={`metric-card ${conflicts ? "warn" : ""} ${showNeedsSyncOnly ? "active" : ""}`}
            onClick={() => setShowNeedsSyncOnly((value) => !value)}
            title="Servers whose target client configs do not match the source config"
            type="button"
          >
            <strong>{conflicts}</strong>
            <span>Needs sync</span>
          </button>
        </div>

        <div className="toolbar">
          <button onClick={newServer} title="New server" type="button">
            <Plus size={16} />
          </button>
          <button onClick={openJsonImport} title="Import JSON" type="button">
            <ClipboardPaste size={16} />
          </button>
          <button onClick={openRawConfigs} title="Raw MCP configs" type="button">
            <FileCode size={16} />
          </button>
          <button onClick={showOverview} title="Overview" type="button">
            <House size={16} />
          </button>
          <button onClick={loadData} title="Refresh" type="button">
            <RefreshCw size={16} />
          </button>
          <button onClick={syncAllClients} title="Sync all clients" type="button">
            <UploadCloud size={16} />
          </button>
        </div>

        <div className="server-list">
          {loading ? (
            <div className="empty-state">Loading MCP configs...</div>
          ) : servers.length === 0 ? (
            <div className="empty-state">No MCP servers found.</div>
          ) : visibleServers.length === 0 ? (
            <div className="empty-state">No servers need sync.</div>
          ) : (
            visibleServers.map((entry) => (
              <button
                className={`server-row ${entry.name === selectedName ? "active" : ""}`}
                key={entry.name}
                onClick={() => openServer(entry)}
                type="button"
              >
                <span className="server-title">
                  {entry.name}
                  {entry.conflict && <span className="sync-badge">Needs sync</span>}
                </span>
                <span className="client-dots">
                  {clients.map((client) => (
                    <i
                      className={entry.targetClients.includes(client.id) ? "on" : ""}
                      key={client.id}
                      title={client.label}
                    />
                  ))}
                </span>
              </button>
            ))
          )}
        </div>
      </section>

      <section className="workspace">
        <div className="status-strip">
          {statuses.map((status) => (
            <div className="status-pill" key={status.client} title={status.path}>
              <span
                className={
                  status.error
                    ? "status-dot error"
                    : status.exists
                      ? "status-dot ok"
                      : "status-dot"
                }
              />
              <strong>{status.label}</strong>
              <em>{status.error ? "error" : status.exists ? "ready" : "missing"}</em>
            </div>
          ))}
        </div>

        {!editing ? (
          <section className="overview-panel">
            <div className="overview-head">
              <p>Source of truth</p>
              <h2>~/.config/mcp/mcp.json</h2>
              <span>
                Manage the source file directly, then sync it to the selected MCP
                clients from here.
              </span>
            </div>

            {error && (
              <div className="alert error">
                <X size={16} />
                {error}
              </div>
            )}
            {notice && (
              <div className="alert success">
                <Check size={16} />
                {notice}
              </div>
            )}

            <div className="overview-actions">
              <button className="primary" disabled={saving} onClick={syncAllClients} type="button">
                <UploadCloud size={16} />
                Sync All
              </button>
              <button onClick={newServer} type="button">
                <Plus size={16} />
                New Server
              </button>
              <button onClick={openJsonImport} type="button">
                <ClipboardPaste size={16} />
                Import JSON
              </button>
              <button onClick={openRawConfigs} type="button">
                <FileCode size={16} />
                Raw Configs
              </button>
            </div>

            <div className="overview-grid">
              <div>
                <strong>{servers.length}</strong>
                <span>servers in source</span>
              </div>
              <div>
                <strong>{totalEnabled}</strong>
                <span>target bindings</span>
              </div>
              <button
                className={conflicts ? "warn" : ""}
                onClick={() => setShowNeedsSyncOnly(true)}
                type="button"
              >
                <strong>{conflicts}</strong>
                <span>need sync with targets</span>
              </button>
            </div>
          </section>
        ) : (
        <form className="editor" onSubmit={saveServer}>
          <div className="editor-head">
            <div>
              <p>{selected ? "Editing server" : "New server"}</p>
              <h2>{form.name || "Untitled MCP"}</h2>
            </div>
            <div className="editor-actions">
              <span className="save-destination">
                Save source, then sync checked targets
              </span>
              <button
                className="ghost"
                onClick={showOverview}
                title="Back to overview"
                type="button"
              >
                <House size={16} />
              </button>
              <button
                className="ghost"
                onClick={openJsonImport}
                title="Import JSON"
                type="button"
              >
                <ClipboardPaste size={16} />
              </button>
              <button
                className="ghost"
                onClick={openRawConfigs}
                title="Raw MCP configs"
                type="button"
              >
                <FileCode size={16} />
              </button>
              <button
                className="ghost"
                onClick={() => setShowSecrets((value) => !value)}
                title={showSecrets ? "Hide secrets" : "Show secrets"}
                type="button"
              >
                {showSecrets ? <EyeOff size={16} /> : <Eye size={16} />}
              </button>
              <button
                className="danger"
                disabled={saving || !form.name}
                onClick={() => setDeleteConfirmOpen(true)}
                title="Delete from source and selected targets"
                type="button"
              >
                <Trash2 size={16} />
              </button>
              <button className="primary" disabled={saving} type="submit">
                <Save size={16} />
                Save & Sync
              </button>
            </div>
          </div>

          {error && (
            <div className="alert error">
              <X size={16} />
              {error}
            </div>
          )}
          {notice && (
            <div className="alert success">
              <Check size={16} />
              {notice}
            </div>
          )}

          <div className="editor-body">
            <section className="form-section">
              <div className="section-head">
                <p>Server definition</p>
                <span>Written to ~/.config/mcp/mcp.json</span>
              </div>

              <div className="name-grid">
                <label>
                  <span>Name</span>
                  <input
                    autoComplete="off"
                    onChange={(event) => setForm({ ...form, name: event.target.value })}
                    placeholder="context7"
                    required
                    value={form.name}
                  />
                </label>
              </div>

              <div className="command-group">
                <label>
                  <span>Command</span>
                  <input
                    autoComplete="off"
                    onChange={(event) => setForm({ ...form, command: event.target.value })}
                    placeholder="npx"
                    required
                    value={form.command}
                  />
                </label>
                <label>
                  <span>Args</span>
                  <textarea
                    className="args-textarea"
                    onChange={(event) => setForm({ ...form, argsText: event.target.value })}
                    placeholder="-y&#10;@modelcontextprotocol/server-filesystem&#10;/Users/me/project"
                    value={form.argsText}
                  />
                </label>
              </div>

              <div className="pane-grid">
                <label>
                  <span>Env</span>
                  <textarea
                    className={`env-textarea ${!showSecrets ? "masked" : ""}`}
                    onChange={(event) => setForm({ ...form, envText: event.target.value })}
                    placeholder="API_KEY=..."
                    value={
                      showSecrets
                        ? form.envText
                        : form.envText
                            .split("\n")
                            .map((line) => {
                              const index = line.indexOf("=");
                              const key = index > -1 ? line.slice(0, index) : line;
                              return index > -1 && isSecretKey(key) ? `${key}=••••••••` : line;
                            })
                            .join("\n")
                    }
                    onFocus={() => setShowSecrets(true)}
                  />
                </label>
              </div>
            </section>

            <section className="sync-section">
              <div className="section-head">
                <p>Sync to</p>
                <span>Save & Sync writes to checked clients.</span>
              </div>
              <div className="targets">
                {clients.map((client) => (
                  <label key={client.id}>
                    <input
                      checked={form.targets[client.id]}
                      onChange={(event) =>
                        setForm({
                          ...form,
                          targets: { ...form.targets, [client.id]: event.target.checked },
                        })
                      }
                      type="checkbox"
                    />
                    <span>{client.label}</span>
                  </label>
                ))}
              </div>
              <div className="delete-note">
                Delete removes from source and checked clients.
              </div>
              <div className="sync-divider" />
              <div className="section-head compact">
                <p>cc-switch</p>
                <span>Save & Sync updates checked cc-switch agent records after agent configs.</span>
              </div>
              <div className="cc-switch-targets">
                {ccSwitchAgents.map((agent) => (
                  <label key={agent.id}>
                    <input
                      checked={form.ccSwitchTargets[agent.id]}
                      onChange={(event) =>
                        setForm({
                          ...form,
                          ccSwitchTargets: {
                            ...form.ccSwitchTargets,
                            [agent.id]: event.target.checked,
                          },
                        })
                      }
                      type="checkbox"
                    />
                    <DatabaseZap size={14} />
                    <span>{agent.label}</span>
                  </label>
                ))}
              </div>
            </section>
          </div>

          {selected?.conflict && (
            <div className="conflict-panel">
              <strong>Targets out of sync</strong>
              <div>
                {clients.map((client) => {
                  if (!selected.targetClients.includes(client.id)) return null;
                  const config = selected.deployedClients[client.id];
                  const differs = !config || !sameConfig(selected.config, config);
                  return (
                    <span className={differs ? "differs" : ""} key={client.id}>
                      {client.label}
                    </span>
                  );
                })}
              </div>
            </div>
          )}
        </form>
        )}
      </section>

      {rawConfigsOpen && (
        <div className="modal-backdrop" role="presentation">
          <section
            className="import-modal raw-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="raw-configs-title"
          >
            <div className="modal-head">
              <div>
                <p>Raw MCP configs</p>
                <h2 id="raw-configs-title">Agent config view</h2>
              </div>
              <button onClick={closeRawConfigs} title="Close" type="button">
                <X size={16} />
              </button>
            </div>

            {rawConfigsLoading ? (
              <div className="empty-state">Loading raw MCP configs...</div>
            ) : (
              <div className="raw-config-layout">
                <div className="raw-config-tabs" role="tablist" aria-label="Raw MCP configs">
                  {rawConfigs.map((config) => (
                    <button
                      aria-selected={activeRawConfig?.id === config.id}
                      className={activeRawConfig?.id === config.id ? "active" : ""}
                      key={config.id}
                      onClick={() => setActiveRawConfigId(config.id)}
                      role="tab"
                      type="button"
                    >
                      <span>{config.label}</span>
                      {config.error && <em>Error</em>}
                    </button>
                  ))}
                </div>

                <div className="raw-config-view">
                  {activeRawConfig ? (
                    <>
                      <div className="raw-config-path">
                        <span>{activeRawConfig.path}</span>
                      </div>
                      {activeRawConfig.error ? (
                        <div className="alert error">
                          <X size={16} />
                          {activeRawConfig.error}
                        </div>
                      ) : (
                        <pre className="raw-config-pre">{activeRawConfig.content}</pre>
                      )}
                    </>
                  ) : (
                    <div className="empty-state">No raw MCP configs found.</div>
                  )}
                </div>
              </div>
            )}
          </section>
        </div>
      )}

      {jsonImportOpen && (
        <div className="modal-backdrop" role="presentation">
          <section className="import-modal" role="dialog" aria-modal="true" aria-labelledby="import-json-title">
            <div className="modal-head">
              <div>
                <p>Import MCP JSON</p>
                <h2 id="import-json-title">Paste server config</h2>
              </div>
              <button onClick={closeJsonImport} title="Close" type="button">
                <X size={16} />
              </button>
            </div>

            <textarea
              className="import-textarea"
              onChange={(event) => {
                setJsonInput(event.target.value);
                setParsedJsonEntries([]);
              }}
              placeholder={'"playwright": { "command": "npx", "args": ["@playwright/mcp@latest"] },\n"ninebot-doc-api": { "command": "doc-api-extractor-mcp", "args": [] }'}
              value={jsonInput}
            />

            <div className="modal-actions">
              <button onClick={parseJsonImport} type="button">
                Parse
              </button>
              <button
                className="primary"
                disabled={saving || (!jsonInput.trim() && parsedJsonEntries.length === 0)}
                onClick={importAllEntries}
                type="button"
              >
                Import All to Source
              </button>
            </div>

            {parsedJsonEntries.length > 0 && (
              <div className="import-results">
                {parsedJsonEntries.map((entry) => (
                  <div className="import-result-row" key={entry.name}>
                    <div>
                      <strong>{entry.name}</strong>
                      <span>{entry.config.command}</span>
                    </div>
                    <button onClick={() => useImportedEntry(entry)} type="button">
                      Use
                    </button>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>
      )}

      {deleteConfirmOpen && (
        <div className="modal-backdrop" role="presentation">
          <section
            className="import-modal delete-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="delete-server-title"
          >
            <div className="modal-head">
              <div>
                <p>Delete MCP server</p>
                <h2 id="delete-server-title">{form.name}</h2>
              </div>
              <button onClick={() => setDeleteConfirmOpen(false)} title="Close" type="button">
                <X size={16} />
              </button>
            </div>

            <div className="delete-summary">
              <strong>This will delete from:</strong>
              <span>~/.config/mcp/mcp.json</span>
              {deleteTargets.map((client) => (
                <span key={client.id}>{client.label} config</span>
              ))}
              {deleteCcSwitchTargets.map((agent) => (
                <span key={agent.id}>cc-switch {agent.label}</span>
              ))}
            </div>

            {deleteTargets.length === 0 && (
              <div className="alert error">
                <X size={16} />
                Select at least one target client before deleting.
              </div>
            )}

            <div className="modal-actions">
              <button onClick={() => setDeleteConfirmOpen(false)} type="button">
                Cancel
              </button>
              <button
                className="danger-solid"
                disabled={saving || !form.name || deleteTargets.length === 0}
                onClick={removeServer}
                type="button"
              >
                Delete from source & selected targets
              </button>
            </div>
          </section>
        </div>
      )}

    </main>
  );
}

export default App;
