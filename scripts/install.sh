#!/usr/bin/env bash
# DeepSpeed installer — builds release binary and installs as a login daemon
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
CONFIG_DIR="$HOME/.config/deepspeed"
BIN_DIR="$HOME/.local/bin"

echo "=== DeepSpeed Installer ==="
echo

# 1. Build
echo "Building release binary..."
cd "$PROJECT_DIR"
cargo build --release
BINARY="$PROJECT_DIR/target/release/deepspeed"
echo "Binary: $BINARY"

# 2. Copy binary
echo "Installing to $BIN_DIR/deepspeed..."
mkdir -p "$BIN_DIR"
cp "$BINARY" "$BIN_DIR/deepspeed"
chmod +x "$BIN_DIR/deepspeed"

# Add ~/.local/bin to PATH in shell profile if not already there
for profile in "$HOME/.zshrc" "$HOME/.zprofile" "$HOME/.bash_profile"; do
    if [ -f "$profile" ] && ! grep -q '\.local/bin' "$profile"; then
        echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$profile"
        echo "Added ~/.local/bin to PATH in $profile"
    fi
done

# 3. Install default config
if [ ! -f "$CONFIG_DIR/deepspeed.toml" ]; then
    echo "Installing default config to $CONFIG_DIR/..."
    mkdir -p "$CONFIG_DIR"
    cp "$PROJECT_DIR/config/deepspeed.toml" "$CONFIG_DIR/deepspeed.toml"
    echo
    echo "IMPORTANT: Edit $CONFIG_DIR/deepspeed.toml to set your Anthropic API key"
    echo "  or set the ANTHROPIC_API_KEY environment variable."
    echo
else
    echo "Config already exists at $CONFIG_DIR/deepspeed.toml — skipping."
fi

# 4. Install launchd agent
echo "Installing launchd agent..."
"$BIN_DIR/deepspeed" install

echo
echo "=== Installation complete ==="
echo
echo "Commands:"
echo "  deepspeed status    — show current system snapshot"
echo "  deepspeed config    — show active configuration"
echo "  deepspeed install   — (re)install launchd agent"
echo "  deepspeed uninstall — stop and remove daemon"
echo
echo "Logs: ~/Library/Logs/deepspeed.log"
