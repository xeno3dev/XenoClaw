# XenoClaw Installation Guide

## Quick Start (Local Dev)

```bash
./install.sh --local
```

This runs `cargo install` and puts `xenoclaw` in `~/.cargo/bin/`. Then:

```bash
xenoclaw -s            # Run the setup wizard (creates ~/.xenoclaw/)
xenoclaw               # Start the agent runtime
```

---

## Full Install (VPS / Bare-Metal)

```bash
sudo ./install.sh
```

This does everything:
1. Builds the release binary
2. Creates the `xenoclaw` service user
3. Sets up directories (`/opt/xenoclaw`, `/etc/xenoclaw`, `/var/lib/xenoclaw`, `/var/log/xenoclaw`)
4. Installs the binary to `/opt/xenoclaw/bin/`
5. Copies `config.example.toml` to `/etc/xenoclaw/config.toml`
6. Builds the Web UI (requires Node.js ≥ 18) and deploys to `/opt/xenoclaw/web/`
7. Installs and enables the systemd service

After install:

```bash
# Edit config with your LLM provider key
sudo nano /etc/xenoclaw/config.toml

# Start the agent
sudo systemctl start xenoclaw-agent

# Check it's running
sudo systemctl status xenoclaw-agent
sudo journalctl -u xenoclaw-agent -f
```

---

## Docker

```bash
cd deploy
docker compose up -d
```

This starts the agent, web UI, Nginx reverse proxy, and Prometheus.

---

## CLI Reference

| Usage | Description |
|-------|-------------|
| `xenoclaw` | Start the agent runtime (default) |
| `xenoclaw -s` / `xenoclaw --setup` | Interactive setup wizard |
| `xenoclaw --reset-key` | Generate a new admin API key |
| `xenoclaw -c /path/to/config.toml` | Custom config path |
| `xenoclaw mcp` | Run as MCP server over stdio (Claude Code / Copilot) |

---

## Service Management

| Command | Description |
|---------|-------------|
| `systemctl start xenoclaw-agent` | Start the agent |
| `systemctl stop xenoclaw-agent` | Stop the agent |
| `systemctl restart xenoclaw-agent` | Restart the agent |
| `systemctl reload xenoclaw-agent` | Hot-reload config (SIGHUP) |
| `journalctl -u xenoclaw-agent -f` | Tail logs |

---

## Configuration

Default: `/etc/xenoclaw/config.toml` (or `~/.xenoclaw/config.toml` for local dev).

Environment variable overrides use `XENOCLAW_` prefix with `__` for nesting:

```bash
XENOCLAW_SERVE__PORT=3000 xenoclaw
```

### Hot-Reloadable (SIGHUP / `systemctl reload`)

- `serve.rate_limit_per_minute`
- `monitoring.log_level`, `log_retention_days`, `max_log_file_size_mb`, `metrics_enabled`, `alert_rules`
- `plugins.enabled`
- `mcp.server_enabled`, `mcp.servers`

### Requires Restart

- Bind addresses/ports (`serve.host`, `serve.port`, `web.*`, `monitoring.metrics_port`)
- `llm.providers`
- `security.*`
- `mcp.server_transport`, `mcp.server_port`
- Database path, TLS certificates

---

## Deploy Folder

Example configs for Nginx, Docker, Prometheus, and systemd are all in `deploy/`:

```
deploy/
├── docker-compose.yml          # Full stack (agent + web + nginx + prometheus)
├── Dockerfile.agent            # Agent container
├── Dockerfile.web              # Web UI container (nginx + static files)
├── nginx.conf                  # Nginx config for Docker Compose
├── nginx-site.conf             # Nginx site config for bare-metal
├── nginx-web.conf              # Nginx config inside the web container
├── prometheus.yml              # Prometheus scrape config
├── xenoclaw-agent.service      # systemd unit file
└── INSTALL.md                  # This file
```

Adapt these to your setup. The install script handles the common case automatically.

---

## Uninstall

```bash
sudo systemctl stop xenoclaw-agent
sudo systemctl disable xenoclaw-agent
sudo rm /etc/systemd/system/xenoclaw-agent.service
sudo systemctl daemon-reload
sudo userdel xenoclaw
sudo rm -rf /opt/xenoclaw /etc/xenoclaw /var/lib/xenoclaw /var/log/xenoclaw
```
