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

# Quickly build & run a branch or PR for testing
xenoclaw try <branch-name>        # branch → install + restart the service
xenoclaw try 42                   # all-digits → PR #42 (fetched via pull/42/head)
xenoclaw try my-branch --release  # release build
xenoclaw try my-branch --exec     # run in the foreground instead of via systemd
xenoclaw try my-branch --no-run   # build only, print binary path
xenoclaw try my-branch --desktop  # build the Tauri desktop app from the ref
xenoclaw try my-branch --desktop --exec    # …and launch it live (tauri dev)
xenoclaw try --restore            # swap the backed-up binary back + restart

# Build & install the desktop app for the current user
xenoclaw install desktop          # build installers + install (AppImage→~/.local, or .deb)
xenoclaw install desktop --build-only   # build only, print the bundle path
xenoclaw install desktop --clone --ref main  # no checkout? fetch source via git first
```

When run outside a checkout (e.g. from a prebuilt binary), `install desktop`
clones the repo into `~/.xenoclaw/desktop-src` and builds from there (`--ref`
picks the branch/tag/PR, `--repo` the remote, `--clone` forces a fresh clone
even when a local checkout exists).

`xenoclaw try` checks the ref out into a dedicated git worktree under
`~/.xenoclaw/try-worktrees/<label>/` (your current checkout is never touched)
and builds the binary there. Worktrees are reused across runs so cargo's
incremental cache survives. Two run modes:

- **Default (service):** installs the fresh binary over `/opt/xenoclaw/bin/xenoclaw`
  (the path `deploy/xenoclaw-agent.service` runs) and restarts `xenoclaw-agent`,
  then probes the configured port for readiness. The new version persists and
  logs to journald (`journalctl -u xenoclaw-agent -f`). Privileged steps use
  `sudo` when not already root.
- **`--exec`:** stops the service (so `Restart=on-failure` can't reclaim the
  port), frees the API port from any stray instance (SIGTERM → SIGKILL), then
  execs the fresh build in the foreground — logs in your terminal, Ctrl-C stops
  it, the installed service binary left untouched.

Linux-only (the port→PID lookup reads `/proc`).

### Frontend

```bash
cd web
npm install
npm run dev      # dev server at :5173, proxies /api to :3000
npm run build    # production build → web/dist/
npm run typecheck
```

The production binary serves `web/dist/` as a fallback on the same port as the API (default `:3000`). Set `XENOCLAW_WEB_DIR` or configure `[web] dir` in config.toml to override.

### Desktop app (Tauri)

`web/src-tauri/` is a **Tauri 2** desktop shell (Windows + Linux) that reuses the
same React frontend as the web UI — it is an independent Cargo workspace
(`[workspace]` table in its `Cargo.toml`), so it is *not* part of `cargo build
--workspace` for `crates/*`. The frontend detects Tauri at runtime via
`isTauri()` (`web/src/lib/tauri.ts`); on the plain web build every desktop path
is inert, so the web UI and TUI are unaffected.

```bash
cd web
npm run icons:generate     # generate the app icon set (dependency-free Node script)
npm run sidecar:build      # build `xenoclaw` and stage it as a Tauri sidecar (local mode)
npm run tauri:dev          # run the desktop app against the Vite dev server
npm run tauri:build        # build NSIS / .deb / AppImage installers
```

CLI shortcuts (wrap the npm flow; `crates/xenoclaw/src/desktop.rs`):
- `xenoclaw install desktop` — runs the whole pipeline (npm install → icons →
  sidecar → `tauri build`) and installs the result for the current user
  (AppImage → `~/.local` with a `.desktop` launcher, or a `.deb` via dpkg).
  `--build-only` stops after building; `--dir` points at the repo/web dir. With
  no local checkout it fetches the source via git into `~/.xenoclaw/desktop-src`
  (`--ref`/`--repo`/`--clone`), so prebuilt-binary users can build too. The
  pipeline pre-checks `node_modules`/`target` ownership and bails with a fix
  hint if a prior `sudo` run left them root-owned.
- `xenoclaw try <ref> --desktop` — builds the desktop app from a branch/PR
  worktree (`--exec` → `tauri dev`, `--no-run` → frontend + sidecar only).

Key pieces:
- **Backend URL routing** — `web/src/lib/backend.ts` owns server *profiles*, the
  active server, theme, and per-server tokens (localStorage). `getApiBase()`
  returns `''` on the web (relative same-origin URLs, unchanged) or the active
  profile's URL in the desktop app. `useAuth.apiFetch` and `useWebSocket`
  resolve through it, so existing `/api/...` callers work in both targets.
- **Local backend** — `src-tauri/src/sidecar.rs` spawns `xenoclaw serve` bound to
  `127.0.0.1:<free-port>` using the `XENOCLAW_API_PORT` / `XENOCLAW_API_HOST`
  env overrides (applied in `xenoclaw serve`); the frontend health-gates the
  connection.
- **Native shell** — `src-tauri/src/lib.rs`: custom titlebar (window control
  commands), tray (show/hide/quit, close-hides-to-tray), native notifications,
  window-state persistence, and the auto-updater.
- **Updater** — configured in `tauri.conf.json` (placeholder pubkey from
  `icons:generate`); the `desktop-build` GitHub workflow signs artifacts and
  publishes `latest.json` on tag pushes. See `web/src-tauri/README.md`.

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
| PUT | /api/v1/config | Update config (mode, system_prompt, log_level, rate_limit_default, provider) |
| GET | /api/v1/sessions | List chat sessions (from the `sessions` SQLite table) |
| DELETE | /api/v1/sessions/:id | Delete a session and its upload directory |
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
web endpoint and the messaging bridges). Uploads larger than `[uploads]
max_upload_size` (bytes, default 10 MiB) are rejected by both `common::uploads`
and the web upload handler.

**Web flow:** the Chat page POSTs files to `/api/v1/uploads/{session}` (multipart),
then includes the returned workspace-relative paths in the WS `message` payload's
`attachments` array. The WS handler appends a note to the message content listing
the files and which tool to use, then forwards to `AgentCore::process_message`.

**Vision:** the `view_image` tool loads an image and — when the active model
supports vision — returns an image sentinel (`{"__xeno_image__": {...}}`, key in
`llm_router::types::IMAGE_SENTINEL_KEY`). The agent core converts that sentinel
into a multimodal `ChatMessage`; the Anthropic/OpenAI/Gemini providers serialize the
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

## LLM Providers

Providers live in `llm-router/src/providers/` and are wired through `factory.rs`
from the `[providers]` config section (each entry sets `provider = "<name>"`).
Currently implemented:

| Provider | `provider` value | Notable models | Vision |
|----------|------------------|----------------|--------|
| Anthropic | `anthropic` | Claude family | yes |
| OpenAI | `openai` | GPT family | yes |
| Google Gemini | `gemini` | `gemini-1.5-pro`, `gemini-1.5-flash` | yes |

The active provider can be switched at runtime via `PUT /api/v1/config`
(`provider` key) or the TUI `/model` command (see below). To add a provider,
implement `LlmProvider` (including `supports_vision()` and `build_request_body`),
register it in `providers/mod.rs` + `factory.rs`, and extend the config schema.

## TUI Commands

The TUI client (`crates/tui`) supports slash commands (`crates/tui/src/commands.rs`):

- `/model` — lists the configured providers and switches the active one by issuing
  `PUT /api/v1/config` with the `provider` key.

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
- `{ type: "tool_call", payload: { name, status } }` — live tool progress; `status` is `"started"` \| `"finished"` \| `"error"` (emitted from `AgentEvent::ToolCall`)
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
| `provider` | name of the LLM provider to switch to (must be present in `[providers]`) | Calls `LlmRouter::set_active_provider`; unknown names are reported in `warnings` |

Unknown keys are reported in the response's `warnings` array but don't fail the request.

## Wizard token validation

After the user enters Telegram / Discord / WhatsApp credentials in the setup wizard, an inline validation step runs:

- **Telegram**: GET `https://api.telegram.org/bot<token>/getMe`, expects `ok: true`
- **Discord**: GET `https://discord.com/api/v10/users/@me` with `Authorization: Bot <token>`, expects 200
- **WhatsApp**: local E.164 format check (`+` followed by 7–15 digits, leading digit 1–9)

Failures don't block submission — the user can press Enter to save anyway (useful when the host is temporarily offline) or Esc to go back and fix the input.

## Resource metrics

CPU and memory percentages on the Dashboard come from a `/proc` sampler (Linux-only) that runs every 2s in a background tokio task. The values land in `Arc<RwLock<ResourceMetrics>>`, which the `/api/v1/status` handler reads — no per-request sampling cost.

## Rate Limiting

Rate limiting is applied as per-route Axum middleware (`api-server/src/middleware.rs`),
not globally in the request handler. The `/api/v1/health` and `/api/v1/auth/login`
routes are exempt. The per-key limit is `rate_limit_default` (runtime-changeable;
see above) and lives in `RateLimiter` (`security-layer/src/rate_limit.rs`).

## Task Scheduler

The cron parser (`task-scheduler/src/cron.rs`) accepts standard 5-field cron
expressions plus these macros: `@hourly`, `@daily` (alias `@midnight`),
`@weekly`, `@monthly`, and `@yearly` (alias `@annually`). Macros are expanded to
their equivalent 5-field expression before scheduling.

## Memory Store

`memory-store` is SQLite-backed and supports semantic search in addition to the
plain memory tools: `MemoryStore::embed` produces an embedding for a text, and
`semantic_search(query_embedding, limit)` returns the closest stored memories by
cosine similarity (`MemoryHit`).

## Sessions

Chat sessions are tracked in the `sessions` SQLite table (`memory-store/src/sessions.rs`),
exposed via `GET /api/v1/sessions` (list) and `DELETE /api/v1/sessions/:id`.
Deleting a session also removes that session's `{workspace}/uploads/{session_id}/`
directory.

## Plugin toggles

The Plugins page persists enabled/disabled state to the `plugin_states` SQLite table. On restart, the agent re-applies the saved state by unloading any plugin that the user previously disabled. Toggling a plugin on calls `PluginManager::reload_plugin` (which loads it from disk); toggling off calls `unload_plugin`.
