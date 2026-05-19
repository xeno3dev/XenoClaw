# XenoClaw Installation Guide

## Quick Start (Local)

```bash
# One-liner: builds and installs to ~/.cargo/bin/
./install.sh

# Or manually:
cargo install --path crates/xenoclaw
```

Now `xenoclaw` works from anywhere:

```bash
xenoclaw -s            # Run the setup wizard (creates config.toml in cwd)
xenoclaw               # Start the agent runtime
```

**That's it.** The `-s` flag walks you through provider selection, API key, workspace init, server bind, and sandbox config — then offers to launch the runtime immediately.

---

## CLI Reference

| Usage | Description |
|-------|-------------|
| `xenoclaw` | Start the agent runtime (default) |
| `xenoclaw -s` / `xenoclaw --setup` | Run the interactive setup wizard |
| `xenoclaw --reset-key` | Generate a new admin API key |
| `xenoclaw -c /path/to/config.toml` | Use a custom config path |
| `xenoclaw serve` | Explicit serve subcommand |
| `xenoclaw setup` | Subcommand form of setup |
| `xenoclaw reset-key` | Subcommand form of reset-key |

---

## Bare-Metal / VPS Deployment

### Prerequisites

- Linux system with systemd
- Rust toolchain (≥ 1.75) or a pre-built binary
- Network connectivity for LLM provider access

### 1. Create the service user

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin xenoclaw
```

### 2. Create directories

```bash
sudo mkdir -p /opt/xenoclaw/bin
sudo mkdir -p /etc/xenoclaw
sudo mkdir -p /var/lib/xenoclaw
sudo mkdir -p /var/log/xenoclaw
sudo mkdir -p /tmp/xenoclaw

sudo chown xenoclaw:xenoclaw /var/lib/xenoclaw /var/log/xenoclaw /tmp/xenoclaw
```

### 3. Build and install the binary

```bash
# From the project root
cargo build --release
sudo cp target/release/xenoclaw /opt/xenoclaw/bin/
sudo chmod 755 /opt/xenoclaw/bin/xenoclaw
```

Or install directly:

```bash
cargo install --path crates/xenoclaw --root /opt/xenoclaw
```

### 4. Configure

**Option A — Interactive wizard (recommended for first install):**

```bash
sudo -u xenoclaw /opt/xenoclaw/bin/xenoclaw -s -c /etc/xenoclaw/config.toml
```

**Option B — Manual config:**

```bash
sudo cp config.example.toml /etc/xenoclaw/config.toml
sudo chmod 640 /etc/xenoclaw/config.toml
sudo chown root:xenoclaw /etc/xenoclaw/config.toml
# Edit with your LLM provider keys, host/port, security settings
sudo nano /etc/xenoclaw/config.toml
```

### 5. Install the systemd service

```bash
sudo cp deploy/xenoclaw-agent.service /etc/systemd/system/
sudo systemctl daemon-reload
```

### 6. Enable and start

```bash
sudo systemctl enable xenoclaw-agent
sudo systemctl start xenoclaw-agent
```

### 7. Verify

```bash
sudo systemctl status xenoclaw-agent
sudo journalctl -u xenoclaw-agent -f
```

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

Default location: `./config.toml` (or specify with `-c /path/to/config.toml`).

### Key settings

```toml
[api]
host = "0.0.0.0"          # Bind address (0.0.0.0 = all interfaces)
port = 9090                # API server port
rate_limit_per_minute = 100

[web]
host = "0.0.0.0"
port = 8080                # Web dashboard port

[monitoring]
log_level = "info"         # debug | info | warn | error | fatal
metrics_port = 9100        # Prometheus metrics
```

All settings can be overridden via environment variables with the `XENOCLAW_` prefix:

```bash
XENOCLAW_API__HOST=127.0.0.1 XENOCLAW_API__PORT=3000 xenoclaw
```

### Hot-Reloadable Settings (no restart needed)

Send SIGHUP or `systemctl reload xenoclaw-agent`:

- `api.rate_limit_per_minute`
- `monitoring.log_level`, `log_retention_days`, `max_log_file_size_mb`
- `monitoring.metrics_enabled`, `alert_rules`
- `plugins.enabled`

### Settings Requiring Full Restart

- `api.host`, `api.port`, `web.host`, `web.port`, `monitoring.metrics_port`
- `general.data_dir`, `general.log_dir`
- `llm.providers`
- `security.*`
- TLS certificates, database path

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
