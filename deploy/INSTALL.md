# XenoClaw Bare-Metal Installation Guide

This guide covers installing XenoClaw as a systemd service on a bare-metal or VPS server.

## Prerequisites

- Linux system with systemd
- Rust toolchain (for building from source) or a pre-built binary
- Network connectivity for LLM provider access

## 1. Create the service user

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin xenoclaw
```

## 2. Create directories

```bash
sudo mkdir -p /opt/xenoclaw/bin
sudo mkdir -p /etc/xenoclaw
sudo mkdir -p /var/lib/xenoclaw
sudo mkdir -p /var/log/xenoclaw
sudo mkdir -p /tmp/xenoclaw

sudo chown xenoclaw:xenoclaw /var/lib/xenoclaw /var/log/xenoclaw /tmp/xenoclaw
```

## 3. Build and install the binary

```bash
# From the project root
cargo build --release
sudo cp target/release/xenoclaw-agent /opt/xenoclaw/bin/
sudo chmod 755 /opt/xenoclaw/bin/xenoclaw-agent
```

## 4. Install configuration

```bash
# Copy and edit the example config
sudo cp deploy/config.example.toml /etc/xenoclaw/config.toml
sudo chmod 640 /etc/xenoclaw/config.toml
sudo chown root:xenoclaw /etc/xenoclaw/config.toml
```

Edit `/etc/xenoclaw/config.toml` with your LLM provider keys, security settings, and other options.

## 5. Install the systemd service

```bash
sudo cp deploy/xenoclaw-agent.service /etc/systemd/system/
sudo systemctl daemon-reload
```

## 6. Enable and start the service

```bash
# Enable auto-start on boot
sudo systemctl enable xenoclaw-agent

# Start the service
sudo systemctl start xenoclaw-agent
```

## 7. Verify

```bash
# Check service status
sudo systemctl status xenoclaw-agent

# View logs
sudo journalctl -u xenoclaw-agent -f
```

## Service Management

| Command | Description |
|---------|-------------|
| `systemctl start xenoclaw-agent` | Start the agent |
| `systemctl stop xenoclaw-agent` | Stop the agent |
| `systemctl restart xenoclaw-agent` | Restart the agent |
| `systemctl reload xenoclaw-agent` | Reload configuration (SIGHUP) |
| `journalctl -u xenoclaw-agent` | View logs |
| `journalctl -u xenoclaw-agent --since "1 hour ago"` | Recent logs |

## Auto-Restart Behavior

The service is configured with `Restart=on-failure` and `RestartSec=5`, meaning:

- If the agent crashes or exits with a non-zero code, systemd will restart it after 5 seconds
- The service starts after the network is online (`After=network-online.target`)
- Security hardening is applied via `ProtectSystem=strict` and `ProtectHome=true`

## Zero-Downtime Configuration Reload

The agent supports hot-reloading a subset of configuration settings via SIGHUP:

```bash
# Reload configuration without restarting
sudo systemctl reload xenoclaw-agent
```

### Hot-Reloadable Settings (no restart needed)

| Setting | Description |
|---------|-------------|
| `api.rate_limit_per_minute` | API rate limiting |
| `monitoring.log_level` | Log verbosity |
| `monitoring.log_retention_days` | Log retention period |
| `monitoring.max_log_file_size_mb` | Log rotation threshold |
| `monitoring.metrics_enabled` | Metrics collection toggle |
| `monitoring.alert_rules` | Alert rule definitions |
| `plugins.enabled` | Plugin system toggle |

### Settings Requiring Full Restart

| Setting | Reason |
|---------|--------|
| `api.host`, `api.port` | Bound socket cannot change at runtime |
| `web.host`, `web.port` | Bound socket cannot change at runtime |
| `monitoring.metrics_port` | Bound socket cannot change at runtime |
| `general.data_dir`, `general.log_dir` | Data directories are opened at startup |
| `llm.providers` | Provider connections are established at startup |
| `security.*` | Security rules affect active sandboxes |
| TLS certificates | Loaded at server startup |
| Database path | Connection opened at startup |

## Uninstall

```bash
sudo systemctl stop xenoclaw-agent
sudo systemctl disable xenoclaw-agent
sudo rm /etc/systemd/system/xenoclaw-agent.service
sudo systemctl daemon-reload
sudo userdel xenoclaw
sudo rm -rf /opt/xenoclaw /etc/xenoclaw /var/lib/xenoclaw /var/log/xenoclaw
```
