```
██╗  ██╗███████╗███╗   ██╗ ██████╗  ██████╗██╗      █████╗ ██╗    ██╗
╚██╗██╔╝██╔════╝████╗  ██║██╔═══██╗██╔════╝██║     ██╔══██╗██║    ██║
 ╚███╔╝ █████╗  ██╔██╗ ██║██║   ██║██║     ██║     ███████║██║ █╗ ██║
 ██╔██╗ ██╔══╝  ██║╚████║║██║   ██║██║     ██║     ██╔══██║██║███╗██║
██╔╝ ██╗███████╗██║ ╚███║ ╚██████╔╝╚██████╗███████╗██║  ██║╚███╔███╔╝
╚═╝  ╚═╝╚══════╝╚═╝  ╚══╝ ╚═════╝  ╚═════╝╚══════╝╚═╝  ╚═╝ ╚══╝╚══╝
```

**Self-hosted AI agent runtime for VPS/bare-metal. Always-on. Dual-mode. Your infrastructure.**

---

## What is XenoClaw?

XenoClaw is a modular, always-on AI agent system you deploy on your own server. It runs 24/7, handles tasks autonomously, and gives you two operational modes you can hot-swap between:

- **General Agent** — Task automation, scheduling, monitoring, knowledge management, and conversational AI accessible via API, TUI, web dashboard, or messaging platforms.
- **Coding Agent** — Everything above plus file editing, terminal access, git operations, LSP integration, and visual diffs. Think OpenCode/Aider but running as a persistent daemon on your VPS.

Switch between modes at runtime. No restart needed.

---

## Features

### Dual Agent Mode
| Mode | Capabilities |
|------|-------------|
| **General** | LLM chat, task scheduling, knowledge store, plugin tools, messaging bridge |
| **Coding** | + File ops with undo history, shell execution, git, LSP diagnostics, diff rendering |

### Multi-Provider LLM Failover
27 providers supported with automatic priority-based failover:

Anthropic · Google Gemini · OpenAI · AWS Bedrock · OpenRouter · Together AI · Mistral AI · Fireworks AI · DeepSeek · Groq · xAI · Perplexity · Cohere · AI21 Labs · Hugging Face · Replicate · Requesty · Cerebras · SambaNova · Ollama · vLLM · LM Studio · Qwen · MiniMax · Zhipu AI · Moonshot AI · Baidu Qianfan

If your primary provider goes down, XenoClaw routes to the next one automatically. No dropped requests.

### Messaging Bridge
Chat with your agent from anywhere:
- **Telegram** — Bot API integration
- **Discord** — Full bot with slash commands
- **WhatsApp** — Web bridge

Messages flow through the same agent core, same tools, same memory.

### Always-On Architecture
- Process supervisor with auto-restart (< 10s recovery)
- Health checks every 15 seconds
- SIGHUP hot-reload for config changes (no downtime)
- Session state persistence across restarts
- Cron + event-driven task scheduler

### Security
- Filesystem & network sandboxing with allowlists
- API key authentication (SHA-256 hashed, never logged)
- RBAC (Admin / Operator roles)
- Brute-force protection with lockout
- Resource limits (memory, CPU, process count)
- Full audit trail

### Plugin System
- WASM-sandboxed plugins with hot-reload
- Register custom tools, event handlers, memory scopes
- Plugin manifest with capability permissions
- Fault isolation — a broken plugin never crashes the agent

### Observability
- Prometheus metrics endpoint
- JSON structured logging (tracing)
- Configurable alert rules
- Log rotation and retention policies

---

## Quick Start

```bash
# Install (requires Rust toolchain)
./install.sh
# or: cargo install --path crates/xenoclaw

# Run the setup wizard
xenoclaw -s

# Start the runtime
xenoclaw
```

The setup wizard walks you through provider selection, API key generation, workspace initialization, and server configuration in 8 interactive steps.

---

## Usage

```bash
xenoclaw              # Start the agent runtime (default: reads ./config.toml)
xenoclaw -s           # Interactive setup wizard
xenoclaw --setup      # Same as -s
xenoclaw --reset-key  # Generate a new admin API key
xenoclaw -c /path/to/config.toml  # Custom config path
```

### Subcommands (alternative syntax)

```bash
xenoclaw serve        # Start the runtime
xenoclaw setup        # Setup wizard
xenoclaw reset-key    # New API key
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      External Interfaces                      │
│   Web UI (React)  ·  TUI (Ratatui)  ·  Telegram/Discord/WA │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────▼──────────────────────────────────┐
│                    API Server (Axum)                          │
│              REST + WebSocket · Auth · Rate Limiting          │
└──────────────────────────┬──────────────────────────────────┘
                           │
┌──────────────────────────▼──────────────────────────────────┐
│                      Agent Core                              │
│  ┌─────────┐  ┌───────────┐  ┌──────────┐  ┌───────────┐  │
│  │LLM Router│  │Tool Registry│  │Event Bus │  │Session Mgr│  │
│  └─────────┘  └───────────┘  └──────────┘  └───────────┘  │
└───────┬──────────────┬──────────────┬───────────────────────┘
        │              │              │
   ┌────▼────┐   ┌────▼────┐   ┌────▼────┐
   │ Coding  │   │ Plugin  │   │  Task   │
   │ Module  │   │ System  │   │Scheduler│
   │(optional)│   │ (WASM)  │   │ (cron)  │
   └─────────┘   └─────────┘   └─────────┘
```

Built in Rust. 12 workspace crates. ~512MB idle memory footprint.

---

## Configuration

XenoClaw uses TOML configuration with environment variable overrides (`XENOCLAW_` prefix):

```toml
[general]
agent_name = "xenoclaw"

[[llm.providers]]
name = "anthropic"
provider_type = "anthropic"
api_key = ""  # or set XENOCLAW_LLM__PROVIDERS__0__API_KEY
model = "claude-sonnet-4-20250514"
priority = 1

[api]
host = "0.0.0.0"
port = 9090

[security.resource_limits]
max_memory_mb = 512
max_cpu_percent = 80
```

See [`config.example.toml`](config.example.toml) for the full reference.

---

## Deployment

### Local development
```bash
xenoclaw -s && xenoclaw
```

### VPS / bare-metal (systemd)
```bash
sudo cp target/release/xenoclaw /opt/xenoclaw/bin/
sudo cp deploy/xenoclaw-agent.service /etc/systemd/system/
sudo systemctl enable --now xenoclaw-agent
```

### Docker
```bash
docker compose -f deploy/docker-compose.yml up -d
```

See [`deploy/INSTALL.md`](deploy/INSTALL.md) for the full installation guide.

---

## Project Structure

```
crates/
├── xenoclaw/            # Binary entry point + setup wizard
├── agent-core/          # Central runtime, message loop, tool execution
├── llm-router/          # Multi-provider routing with failover
├── api-server/          # Axum REST + WebSocket server
├── coding-module/       # File ops, shell, git, LSP, diffs
├── plugin-system/       # WASM plugin loading + sandboxed API
├── task-scheduler/      # Cron, file watchers, webhooks, dependencies
├── security-layer/      # Auth, RBAC, sandbox, rate limiting, audit
├── memory-store/        # SQLite + vector search (sqlite-vec)
├── process-supervisor/  # Health monitoring, auto-restart
├── messaging-integration/ # Telegram, Discord, WhatsApp bridges
├── tui/                 # Terminal UI (Ratatui)
└── common/              # Shared types, config, errors, logging
```

---

## Requirements

- Rust ≥ 1.75 (stable)
- Linux (primary target), macOS (development)
- 512MB RAM minimum (idle)

---

## License

MIT
