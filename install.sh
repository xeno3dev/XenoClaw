#!/bin/bash
# XenoClaw installer
# Builds the binary, sets up directories, installs the systemd service,
# and builds the web UI.
#
# Usage:
#   sudo ./install.sh         # Full install (binary + service + web UI)
#   ./install.sh --local      # Local dev only (just cargo install, no systemd)

set -e

INSTALL_DIR="/opt/xenoclaw"
CONFIG_DIR="/etc/xenoclaw"
DATA_DIR="/var/lib/xenoclaw"
LOG_DIR="/var/log/xenoclaw"
WEB_DIR="/opt/xenoclaw/web"
SERVICE_FILE="/etc/systemd/system/xenoclaw-agent.service"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC} $1"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

# ─── Local-only mode ──────────────────────────────────────────────────────────

if [[ "$1" == "--local" ]]; then
    info "Building XenoClaw (release, local install)..."
    cargo install --path crates/xenoclaw --force
    echo ""
    info "Done. Run 'xenoclaw -s' to start the setup wizard."
    exit 0
fi

# ─── Full VPS/bare-metal install ──────────────────────────────────────────────

if [[ $EUID -ne 0 ]]; then
    error "Full install requires root. Run with sudo or use './install.sh --local' for local dev."
fi

# ─── Step 1: Build the binary ─────────────────────────────────────────────────

info "Building XenoClaw (release)..."
cargo build --release

# ─── Step 2: Create service user ──────────────────────────────────────────────

if ! id -u xenoclaw &>/dev/null; then
    info "Creating service user 'xenoclaw'..."
    useradd --system --no-create-home --shell /usr/sbin/nologin xenoclaw
else
    info "Service user 'xenoclaw' already exists."
fi

# ─── Step 3: Create directories ───────────────────────────────────────────────

info "Creating directories..."
mkdir -p "$INSTALL_DIR/bin"
mkdir -p "$WEB_DIR"
mkdir -p "$CONFIG_DIR"
mkdir -p "$DATA_DIR"
mkdir -p "$LOG_DIR"
mkdir -p /tmp/xenoclaw

chown xenoclaw:xenoclaw "$DATA_DIR" "$LOG_DIR" /tmp/xenoclaw

# ─── Step 4: Install the binary ───────────────────────────────────────────────

# Stop the service before replacing the binary.  On Debian/systemd, cp(1) will
# fail with ETXTBSY if the kernel has the old inode mapped for execution.
# Using `install -m 755` (atomic rename) sidesteps that, but stopping first
# is the belt-and-suspenders fix that also prevents a brief gap where the
# service auto-restarts against a half-written binary.
SERVICE_WAS_ACTIVE=false
if systemctl is-active --quiet xenoclaw-agent 2>/dev/null; then
    info "Stopping xenoclaw-agent service before binary update..."
    systemctl stop xenoclaw-agent
    SERVICE_WAS_ACTIVE=true
fi

info "Installing binary to $INSTALL_DIR/bin/xenoclaw..."
install -m 755 target/release/xenoclaw "$INSTALL_DIR/bin/xenoclaw"

# Ensure xenoclaw is accessible on PATH without manual PATH edits.
ln -sf "$INSTALL_DIR/bin/xenoclaw" /usr/local/bin/xenoclaw

# On Debian/Ubuntu, rustup prepends ~/.cargo/bin to PATH before /usr/local/bin.
# If a previous `cargo install` (e.g. --local mode) left a xenoclaw binary
# there it will shadow the system binary we just installed — "no matter what"
# the user does, they run the old copy.  Overwrite any such copies in-place
# using `install` (atomic rename, safe even if xenoclaw is currently running).
_update_cargo_bin() {
    local home_dir="$1"
    local cargo_bin="${home_dir}/.cargo/bin/xenoclaw"
    [[ -f "$cargo_bin" ]] || return 0
    local uid gid
    uid=$(stat -c '%u' "$cargo_bin")
    gid=$(stat -c '%g' "$cargo_bin")
    info "Updating shadowing cargo binary at $cargo_bin..."
    install -o "$uid" -g "$gid" -m 755 target/release/xenoclaw "$cargo_bin"
}
_update_cargo_bin "/root"
if [[ -n "${SUDO_USER:-}" ]]; then
    _sudo_home=$(getent passwd "${SUDO_USER}" | cut -d: -f6)
    [[ -n "$_sudo_home" ]] && _update_cargo_bin "$_sudo_home"
fi

# ─── Step 5: Install config ───────────────────────────────────────────────────

if [[ ! -f "$CONFIG_DIR/config.toml" ]]; then
    info "Installing default config to $CONFIG_DIR/config.toml..."
    cp config.example.toml "$CONFIG_DIR/config.toml"
    chmod 640 "$CONFIG_DIR/config.toml"
    chown root:xenoclaw "$CONFIG_DIR/config.toml"
    warn "Edit $CONFIG_DIR/config.toml with your LLM provider keys before starting."
else
    info "Config already exists at $CONFIG_DIR/config.toml, skipping."
fi

# ─── Step 6: Build and deploy Web UI ──────────────────────────────────────────

if [[ -d "web" ]]; then
    if command -v npm &>/dev/null; then
        info "Building Web UI..."
        (cd web && npm ci && npm run build)
        info "Deploying Web UI to $WEB_DIR..."
        rm -rf "${WEB_DIR:?}"/*
        cp -r web/dist/* "$WEB_DIR/"
        chown -R root:root "$WEB_DIR"
    else
        warn "npm not found — skipping Web UI build. Install Node.js >= 18 to build the UI."
    fi
else
    warn "web/ directory not found — skipping Web UI build."
fi

# ─── Step 7: Install systemd service ──────────────────────────────────────────

info "Installing systemd service..."
cp deploy/xenoclaw-agent.service "$SERVICE_FILE"
systemctl daemon-reload
systemctl enable xenoclaw-agent

# Restart (or start) the service now that the new binary is in place.
if [[ "$SERVICE_WAS_ACTIVE" == "true" ]]; then
    info "Restarting xenoclaw-agent with the new binary..."
    systemctl start xenoclaw-agent
    info "xenoclaw-agent restarted."
else
    info "Systemd service installed and enabled (will start on boot)."
fi

# ─── Done ─────────────────────────────────────────────────────────────────────

echo ""
info "Installation complete!"
echo ""
echo "  Binary:   $INSTALL_DIR/bin/xenoclaw"
echo "  Config:   $CONFIG_DIR/config.toml"
echo "  Data:     $DATA_DIR/"
echo "  Logs:     $LOG_DIR/"
echo "  Web UI:   $WEB_DIR/"
echo "  Service:  xenoclaw-agent.service"
echo ""
echo "Next steps:"
echo "  1. Edit $CONFIG_DIR/config.toml (add your LLM provider API key) OR run xenoclaw -s for auto/guided setup."
echo "  2. Start the agent:  systemctl start xenoclaw-agent"
echo "  3. Check status:     systemctl status xenoclaw-agent"
echo "  4. View logs:        journalctl -u xenoclaw-agent -f"
echo ""
echo "Example Nginx and Docker configs are in the deploy/ folder."
echo ""
