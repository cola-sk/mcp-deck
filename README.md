# MCP Deck

MCP Deck is a desktop MCP configuration manager built with Tauri, Rust, React, and TypeScript.

## Source of Truth

MCP Deck uses one user-editable MCP source file:

```text
~/.config/mcp/mcp.json
```

Expected shape:

```json
{
  "mcpServers": {
    "context7": {
      "command": "npx",
      "args": ["-y", "@upstash/context7-mcp"],
      "env": {}
    }
  }
}
```

MCP Deck only stores UI/sync metadata in:

```text
~/.config/mcp-deck/state.json
```

## Supported Config Targets

- Antigravity: `~/.gemini/antigravity/mcp_config.json`
- Codex: `~/.codex/config.toml`
- Claude Code: `~/.claude.json`
- VS Code: `~/Library/Application Support/Code/User/mcp.json`

## Development

```bash
npm install
npm run tauri dev
```

## Safety Defaults

- `~/.config/mcp/mcp.json` is the source of truth; target client files are sync outputs.
- Existing JSON server fields are preserved when editing known MCP fields.
- Codex TOML is edited with `toml_edit` to preserve surrounding comments and unrelated settings.
- Existing files are backed up before writes with a `.mcp-deck.bak` suffix.
- One-click sync writes all source servers to their target clients. Servers without MCP Deck metadata default to all supported clients.
- cc-switch synchronization is explicit and writes through a SQLite transaction. It updates cc-switch settings, active providers, and the central `mcp_servers` registry table, ensuring configurations are preserved when saving or switching inside CC Switch.

