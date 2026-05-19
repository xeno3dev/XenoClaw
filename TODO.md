# XenoClaw — Remaining Implementation Work

## 1. MCP Protocol Handler (Server Mode)

XenoClaw needs to speak the MCP JSON-RPC protocol so Claude Code and Copilot can connect to it and use its tools.

**What exists:** Config (`[mcp]` section), ToolRegistry with full tool execution support.

**What's missing:**
- A JSON-RPC message loop that handles MCP protocol methods over stdio or HTTP+SSE
- Must implement these MCP methods:
  - `initialize` — handshake, return server capabilities
  - `tools/list` — return all registered tools with their JSON schemas
  - `tools/call` — execute a tool and return the result
  - `resources/list` — list available resources (memory entries, files, etc.)
  - `resources/read` — read a specific resource
  - `prompts/list` — list available prompt templates
  - `prompts/get` — get a specific prompt template
- Stdio transport: read JSON-RPC from stdin, write to stdout (for `claude --mcp-server xenoclaw`)
- HTTP+SSE transport: HTTP POST for requests, Server-Sent Events for streaming (for remote access)
- Register as a spawnable MCP server in Claude Code's config (`~/.claude/mcp_servers.json`)

**Files to create:**
- `crates/xenoclaw/src/mcp_server.rs` — the protocol handler
- Or a new crate `crates/mcp/` if it grows large

**Dependencies needed:** `serde_json` (already available), possibly `jsonrpc-core` or hand-roll the simple JSON-RPC 2.0 parsing.

---

## 2. MCP Client (Connecting to External MCP Servers)

XenoClaw should be able to spawn and connect to external MCP servers (like filesystem, GitHub, database servers) and register their tools in its own ToolRegistry.

**What exists:** Config (`[[mcp.servers]]` array with command, args, env).

**What's missing:**
- Process spawning: start each configured MCP server as a child process
- Stdio JSON-RPC client: send `initialize`, then `tools/list` to discover tools
- Dynamic tool registration: for each tool discovered, create a proxy Tool impl that calls `tools/call` on the child process
- Lifecycle management: restart crashed servers, handle graceful shutdown
- Health monitoring: detect unresponsive servers

**Files to create:**
- `crates/xenoclaw/src/mcp_client.rs` — spawns servers, discovers tools, proxies calls

---

## 3. Built-in Tool Registration

The ToolRegistry exists and works, but no tools are registered at startup. The agent can't actually DO anything.

**What exists:** ToolRegistry, Tool trait, CodingModule with file_ops/shell/git/lsp.

**What's missing:**
- A startup function in `main.rs` that registers built-in tools:
  - `file_read` — read a file (from coding-module's file_ops)
  - `file_write` — write/create a file
  - `file_edit` — string replacement edit
  - `file_delete` — delete a file
  - `shell_exec` — execute a shell command (from coding-module's shell_executor)
  - `git_status`, `git_diff`, `git_commit` — git operations
  - `memory_search` — semantic search over memory store
  - `memory_store` — store a knowledge entry
  - `task_create` — create a scheduled task
  - `task_list` — list scheduled tasks
- Each tool needs to implement the `Tool` trait (name, description, parameters_schema, execute)
- Tools need access to the CodingModule, MemoryStore, and Scheduler (via Arc references)

**Files to create:**
- `crates/xenoclaw/src/tools/mod.rs` — tool registration function
- `crates/xenoclaw/src/tools/file_tools.rs` — file_read, file_write, file_edit, file_delete
- `crates/xenoclaw/src/tools/shell_tools.rs` — shell_exec
- `crates/xenoclaw/src/tools/git_tools.rs` — git operations
- `crates/xenoclaw/src/tools/memory_tools.rs` — memory_search, memory_store
- `crates/xenoclaw/src/tools/task_tools.rs` — task_create, task_list

---

## 4. Session Persistence and Multi-Session API

Sessions exist as a data model but aren't wired to the API layer for creation, switching, or persistence.

**What exists:** SessionId type, SessionManager in agent-core, SessionState in memory-store, SessionSource enum.

**What's missing:**
- API endpoints for session management:
  - `POST /api/v1/sessions` — create a new session (with source tag)
  - `GET /api/v1/sessions` — list active sessions
  - `GET /api/v1/sessions/:id` — get session details
  - `DELETE /api/v1/sessions/:id` — close a session
- The WebSocket chat endpoint should accept a session_id parameter and route to the correct session
- Session state (conversation history, mode, context) must persist to SQLite via memory-store
- On restart, active sessions should be restored from the database
- CLI providers (Claude Code, Copilot) should each get a unique session tagged with their SessionSource

**Files to modify:**
- `crates/api-server/src/routes/` — add `sessions.rs` route module
- `crates/xenoclaw/src/main.rs` — restore sessions on startup
- `crates/agent-core/src/session_manager.rs` — wire to memory-store for persistence

---

## 5. Messaging Platform Sessions

Each messaging platform user/channel should get their own isolated session with separate history and context.

**What exists:** MessagingIntegration crate with Telegram/Discord/WhatsApp bot scaffolding, SessionSource enum with platform-specific variants.

**What's missing:**
- When a message arrives from Telegram/Discord/WhatsApp:
  1. Look up or create a session for that (platform, user_id/channel_id) pair
  2. Route the message through AgentCore.process_message() with that session
  3. Send the response back to the platform
- Session mapping: `HashMap<(Platform, String), SessionId>` persisted in SQLite
- Each platform bot needs to be started in main.rs and connected to the agent core
- Rate limiting per platform user (separate from API rate limits)

**Files to modify:**
- `crates/messaging-integration/src/` — wire each bot to create/use sessions
- `crates/xenoclaw/src/main.rs` — spawn messaging bots on startup

---

## 6. ~~Tool Calling with CLI Providers~~ ✅ DONE

> Implemented Option A: CLI providers deprecated with documentation. Warning log emitted at
> startup when `claude_code` or `copilot_cli` is configured. README updated with MCP
> integration guide. Users should use an API provider for the LLM backend and connect
> Claude Code / Copilot to XenoClaw via MCP for tool access.

<details>
<summary>Original TODO (archived)</summary>

The `claude_code` and `copilot_cli` provider types currently do text-in/text-out. They can't participate in the tool execution loop.

**What exists:** ClaudeCodeProvider and CopilotCliProvider that shell out and capture stdout.

**What's needed (two options):**

**Option A (Recommended): Use MCP instead.**
Don't use CLI providers as LLM backends. Instead:
- Use a real API provider (Anthropic API, OpenAI API) for the LLM with tool calling
- Use Claude Code / Copilot as MCP clients that connect TO XenoClaw
- This gives full tool support through the proper protocol

**Option B: Parse tool calls from CLI output.**
- Claude Code's `--print` output sometimes contains tool-use XML/JSON blocks
- Parse these from stdout, execute the tools, feed results back via another CLI invocation
- Fragile and not officially supported

**Recommendation:** Remove `claude_code` and `copilot_cli` as LLM provider types. Instead, document that users should:
1. Use an API provider for XenoClaw's LLM backend
2. Connect Claude Code / Copilot to XenoClaw via MCP for tool access

</details>

---

## 7. Plugin Runtime

The plugin system has loading, manifests, and hot-reload detection, but no actual plugin execution environment.

**What exists:** PluginManager, PluginLoader, manifest parsing, directory watching, sandboxed API trait.

**What's missing:**
- WASM runtime integration (Wasmtime) to actually execute plugin code
- Plugin API host functions exposed to WASM (register_tool, register_event_handler, log, memory access)
- At least one example plugin to validate the system works
- Plugin marketplace/registry concept (ClawhubHub mentioned in design)

**Files to create/modify:**
- `crates/plugin-system/src/wasm_runtime.rs` — Wasmtime integration
- `plugins/example-plugin/` — a working example plugin

**Dependencies needed:** `wasmtime` crate (not currently in workspace)

---

## 8. Web UI Completion

The web UI has a login page, chat, and dashboard, but is missing several features.

**What exists:** React app with Login, Chat (WebSocket streaming), Dashboard (polling), Layout with sidebar.

**What's missing:**
- Session switcher UI (list sessions, create new, switch between them)
- Settings page (view/edit config, manage API keys)
- Plugin management page (list, enable/disable, reload)
- Task scheduler UI (create/edit/delete tasks, view history)
- Memory/knowledge browser (search, view, delete entries)
- Agent mode toggle (General ↔ Coding) in the UI
- File diff viewer (render diffs from coding module)
- Mobile responsive testing and polish

---

## 9. Database Initialization

The SQLite schema exists in the design doc but isn't applied at startup.

**What exists:** `memory_store::init_database()` function, schema in design doc.

**What's missing:**
- Call `init_database()` in main.rs on startup
- Ensure the database file is created at `~/.xenoclaw/data/xenoclaw.db`
- Run migrations if the schema changes between versions
- The session_state table needs the SessionSource column

**Files to modify:**
- `crates/xenoclaw/src/main.rs` — call init_database on startup
- `crates/memory-store/src/db.rs` — ensure schema includes session source

---

## 10. SIGHUP Reload for New Config Fields

Hot-reload exists but doesn't cover the new fields added (MCP config, admin credentials).

**What exists:** `spawn_reload_handler()` reloads a subset of config on SIGHUP.

**What's missing:**
- Add MCP server restart on config change (if mcp.server_enabled or port changes)
- Add MCP client reconnection (if [[mcp.servers]] changes)
- Admin credentials should NOT be hot-reloadable (security — require restart)

**Files to modify:**
- `crates/common/src/config/reload.rs` — add MCP fields to ReloadableConfig

---

## Priority Order (recommended)

1. **Built-in tool registration** (#3) — makes the agent functional
2. **MCP server** (#1) — enables Claude Code / Copilot integration
3. **Database init** (#9) — enables persistence
4. **Session persistence** (#4) — enables multi-session
5. **MCP client** (#2) — enables external tool servers
6. **Messaging sessions** (#5) — enables platform-specific sessions
7. **Web UI completion** (#8) — polish
8. **Plugin runtime** (#7) — extensibility
9. **CLI provider decision** (#6) — cleanup
10. **Hot-reload updates** (#10) — operational polish
