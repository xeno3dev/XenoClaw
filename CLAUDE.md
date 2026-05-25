# XenoClaw — Development Guide

## Overview

XenoClaw is a self-hosted AI agent runtime: a Rust backend with an Axum API server, a React/TypeScript web UI, and a TUI client. It supports multiple LLM providers via a router, a plugin system, tool execution, task scheduling, and third-party messaging bridges (Telegram, Discord, WhatsApp).

## Repository Layout

```
XenoClaw/
├── crates/
│   ├── xenoclaw/          # Binary entry point and CLI subcommands
│   ├── api-server/        # Axum HTTP/WebSocket server
│   ├── agent-core/        # Agent runtime: message loop, tool dispatch
│   ├── common/            # Shared types, config models, errors
│   ├── llm-router/        # Multi-provider LLM completion routing
│   ├── plugin-system/     # WASM/native plugin loading
│   ├── security-layer/    # API key auth, bcrypt, rate limiting
│   ├── memory-store/      # SQLite-backed memory tools
│   ├── task-scheduler/    # Cron-style task scheduler
│   ├── process-supervisor/# Agent lifecycle management
│   ├── tui/               # Terminal UI client
│   └── mcp-*/             # MCP client/server bridges
└── web/                   # Vite + React + TypeScript frontend
    └── src/
        ├── components/    # Layout, shared UI
        ├── hooks/         # useAuth, useWebSocket
        └── pages/         # Chat, Dashboard, Settings, Plugins, …
```

## Development Branch

Active development: `claude/web-login-sandbox-setup-MnN5c`

## Build & Run

### Backend

```bash
# From repo root
cargo build --workspace

# Run the agent (reads /etc/xenoclaw/config.toml first, then ~/.xenoclaw/config.toml)
cargo run -p xenoclaw -- serve

# First-run setup wizard
cargo run -p xenoclaw -- setup

# Reset admin password (bypasses TUI wizard)
cargo run -p xenoclaw -- passwd

# Rotate API key
cargo run -p xenoclaw -- set-api-key

# Verify credentials locally
cargo run -p xenoclaw -- verify-login --user admin --password <pw>
```

### Frontend

```bash
cd web
npm install
npm run dev      # dev server at :5173, proxies /api to :3000
npm run build    # production build → web/dist/
npm run typecheck
```

The production binary serves `web/dist/` as a fallback on the same port as the API (default `:3000`). Set `XENOCLAW_WEB_DIR` or configure `[web] dir` in config.toml to override.

## Config File

Default path resolution (preference order):
1. `$XENOCLAW_CONFIG_PATH` (set by the systemd service unit)
2. `/etc/xenoclaw/config.toml` (system install path)
3. `~/.xenoclaw/config.toml` (local dev)

The wizard (`xenoclaw setup`) writes to whichever path you pass it. The systemd service reads `/etc/xenoclaw/config.toml`. Always run the wizard with `--config /etc/xenoclaw/config.toml` on a production install, or use `xenoclaw passwd` to set credentials directly.

## Authentication

Two login methods, both returning a Bearer token:

| Method      | Endpoint             | Credential          |
|-------------|----------------------|---------------------|
| Password    | POST /api/v1/auth/login | username + password (bcrypt) |
| API key     | Bearer header only   | raw key (SHA-256 in config) |

Password tokens are in-memory and lost on server restart. API key tokens are stateless (re-checked against config on each request).

## API Endpoints

All endpoints (except `/api/v1/health` and `/api/v1/auth/login`) require `Authorization: Bearer <token>`.

| Method | Path | Description |
|--------|------|-------------|
| GET | /api/v1/status | Agent status, mode, uptime |
| GET | /api/v1/config | Config subset (version, mode, rate limit) |
| PUT | /api/v1/config | Update config (mode, system_prompt, log_level, rate_limit_default) |
| GET | /api/v1/plugins | List plugins with enabled state |
| POST | /api/v1/plugins/reload | Reload all plugins |
| POST | /api/v1/plugins/:name/toggle | Toggle plugin on/off |
| POST | /api/v1/plugins/:name/reload | Reload specific plugin |
| GET | /api/v1/messaging | Messaging bridge status |
| POST | /api/v1/uploads/:session_id | Multipart file upload → `{workspace}/uploads/{session}/` |
| WS | /api/v1/ws/chat?token=… | Real-time chat stream |
| WS | /api/v1/ws/events?token=… | System event stream |

WebSocket auth uses `?token=` query param (browsers can't send custom headers on WS upgrades).

## File & Image Uploads

Users can attach files/images in the web UI (paperclip button) and via messaging
bridges. Uploads are stored at `{workspace}/uploads/{session_id}/{filename}`
(`common::uploads` owns the path layout + filename sanitization, shared by the
web endpoint and the messaging bridges).

**Web flow:** the Chat page POSTs files to `/api/v1/uploads/{session}` (multipart),
then includes the returned workspace-relative paths in the WS `message` payload's
`attachments` array. The WS handler appends a note to the message content listing
the files and which tool to use, then forwards to `AgentCore::process_message`.

**Vision:** the `view_image` tool loads an image and — when the active model
supports vision — returns an image sentinel (`{"__xeno_image__": {...}}`, key in
`llm_router::types::IMAGE_SENTINEL_KEY`). The agent core converts that sentinel
into a multimodal `ChatMessage`; the Anthropic/OpenAI providers serialize the
`images` field into provider-native content blocks. **Fail-safe:** if the model
is text-only (`LlmRouter::supports_vision()` is false, computed from the primary
provider's model id), `view_image` returns a plain-text explanation instead of
pixels — nothing errors.

To support a new vision provider, implement `LlmProvider::supports_vision()` and
emit image blocks from its `build_request_body`.

**Messaging:** inbound Telegram photos/documents are downloaded and saved to the
session's upload dir via `AgentMessageHandler` (`with_workspace_dir`). The
messaging→agent dispatch itself is still stubbed, so saved files aren't yet fed
into a live agent turn over the bridges — but the storage plumbing is in place.

## Frontend Auth Pattern

All authenticated page fetches must use `apiFetch` from `useAuth()`:

```typescript
const { apiFetch } = useAuth();
const res = await apiFetch('/api/v1/status'); // injects Bearer token, auto-logs out on 401
```

Never call `fetch()` directly from a page that requires login — it won't send the token.

## Agent Modes

Three modes, mirroring Claude Code / OpenCode:

| Mode | Tools available |
|------|----------------|
| General | Base tools only (search, tasks, memory) |
| Plan | Base + read-only dev tools (file read, git status/diff) — destructive tools filtered out |
| Code | Base + all dev tools (file write/edit/delete, shell, git commit) |

Both Plan and Code are `AgentMode::Coding` under the hood; the difference is the `plan_only` flag. When `plan_only` is true, `ToolRegistry::tool_definitions` filters out any tool whose `is_destructive()` returns true (file_create/write/edit/delete, shell_execute, git_commit, memory_store, task_create).

Switch via `PUT /api/v1/config` with `{"settings": {"mode": "general"|"plan"|"code"}}` (`"coding"` is accepted as a back-compat alias for `"code"`). The Chat page header and the Settings page both expose the three-way toggle. Mode change is rejected with `AgentBusy` while a message is in-flight (the UI ignores this and reverts optimistically).

To mark a new tool as state-mutating, override `fn is_destructive(&self) -> bool { true }` in its `Tool` impl — that's all that's needed for Plan mode to hide it.

## WebSocket Message Protocol

All WS messages are JSON `{ type: string, payload: any }`.

**Client → Server:**
- `{ type: "message", payload: { session_id, content } }`

**Server → Client:**
- `{ type: "token", payload: { content } }` — streamed token
- `{ type: "done" }` — stream complete
- `{ type: "error", payload: { message } }` — error
- `{ type: "message", payload: { content, diff_image? } }` — full non-streamed response
- `{ type: "diff_image", payload: { url? | svg? } }` — code diff visualization

## Commit Conventions

```
fix(scope): what was broken and what changed
feat(scope): what was added
style(scope): visual/CSS changes
refactor(scope): no behaviour change
```

## Runtime-changeable settings

The following are wired through `PUT /api/v1/config`:

| Setting | Effect | Notes |
|---------|--------|-------|
| `mode` | `"general"`, `"plan"`, or `"code"` — switches active tool set (`"coding"` = alias for `"code"`) | Rejected silently while a message is streaming (AgentBusy) |
| `system_prompt` | string or null — replaces the prepended prompt | Takes effect on next message; in-flight requests keep old prompt |
| `log_level` | tracing EnvFilter string (`"info"`, `"debug,xenoclaw=trace"`, …) | Reloaded via `tracing_subscriber::reload::Handle` |
| `rate_limit_default` | positive integer — per-key requests per minute | Atomic swap inside `RateLimiter`, takes effect immediately |

Unknown keys are reported in the response's `warnings` array but don't fail the request.

## Wizard token validation

After the user enters Telegram / Discord / WhatsApp credentials in the setup wizard, an inline validation step runs:

- **Telegram**: GET `https://api.telegram.org/bot<token>/getMe`, expects `ok: true`
- **Discord**: GET `https://discord.com/api/v10/users/@me` with `Authorization: Bot <token>`, expects 200
- **WhatsApp**: local E.164 format check (`+` followed by 7–15 digits, leading digit 1–9)

Failures don't block submission — the user can press Enter to save anyway (useful when the host is temporarily offline) or Esc to go back and fix the input.

## Resource metrics

CPU and memory percentages on the Dashboard come from a `/proc` sampler (Linux-only) that runs every 2s in a background tokio task. The values land in `Arc<RwLock<ResourceMetrics>>`, which the `/api/v1/status` handler reads — no per-request sampling cost.

## Plugin toggles

The Plugins page persists enabled/disabled state to the `plugin_states` SQLite table. On restart, the agent re-applies the saved state by unloading any plugin that the user previously disabled. Toggling a plugin on calls `PluginManager::reload_plugin` (which loads it from disk); toggling off calls `unload_plugin`.
