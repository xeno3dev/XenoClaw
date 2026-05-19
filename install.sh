#!/bin/bash
# Install XenoClaw to ~/.cargo/bin/xenoclaw
# After this, `xenoclaw` is available system-wide (assuming ~/.cargo/bin is in PATH).

set -e

echo "Building XenoClaw (release)..."
cargo install --path crates/xenoclaw --force

echo ""
echo "Done. Run 'xenoclaw -s' to start the setup wizard."
