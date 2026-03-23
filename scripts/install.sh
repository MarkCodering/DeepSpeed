#!/usr/bin/env bash
# DeepSpeed installer for macOS and Linux
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
CONFIG_DIR="$HOME/.config/deepspeed"
BIN_DIR="$HOME/.local/bin"

echo "=== DeepSpeed Installer ==="
echo

# Detect OS
OS="$(uname -s)"
case "$OS" in
    Darwin) PLATFORM="macos" ;;
    Linux)  PLATFORM="linux" ;;
    *)
        echo "Unsupported OS: $OS"
        echo "For Windows, use scripts/install.ps1 in PowerShell."
        exit 1
        ;;
esac
echo "Platform: $PLATFORM"
echo

# 1. Build
echo "Building release binary..."
cd "$PROJECT_DIR"
cargo build --release
BINARY="$PROJECT_DIR/target/release/deepspeed"
echo "Binary: $BINARY"
echo

# 2. Install binary to ~/.local/bin
echo "Installing to $BIN_DIR/deepspeed..."
mkdir -p "$BIN_DIR"
cp "$BINARY" "$BIN_DIR/deepspeed"
chmod +x "$BIN_DIR/deepspeed"
# Re-sign with ad-hoc signature — required on Apple Silicon after strip
if [ "$PLATFORM" = "macos" ]; then
    codesign --force --sign - "$BIN_DIR/deepspeed" 2>/dev/null || true
fi

# Add ~/.local/bin to PATH if not already present
for profile in "$HOME/.zshrc" "$HOME/.zprofile" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile"; do
    if [ -f "$profile" ] && ! grep -q '\.local/bin' "$profile"; then
        echo 'export PATH="$HOME/.local/bin:$PATH"' >> "$profile"
        echo "Added ~/.local/bin to PATH in $profile"
    fi
done
echo

# 3. Install default config
if [ ! -f "$CONFIG_DIR/deepspeed.toml" ]; then
    echo "Installing default config to $CONFIG_DIR/..."
    mkdir -p "$CONFIG_DIR"
    cp "$PROJECT_DIR/config/deepspeed.toml" "$CONFIG_DIR/deepspeed.toml"
    echo
    echo "IMPORTANT: Edit $CONFIG_DIR/deepspeed.toml to set your Anthropic API key"
    echo "  or export ANTHROPIC_API_KEY=sk-ant-... in your shell profile."
    echo
else
    echo "Config already exists at $CONFIG_DIR/deepspeed.toml — skipping."
fi

# 4. Install daemon
echo "Installing daemon..."
if [ "$PLATFORM" = "macos" ]; then
    "$BIN_DIR/deepspeed" install
    echo "Log: ~/Library/Logs/deepspeed.log"
elif [ "$PLATFORM" = "linux" ]; then
    # Check systemd --user is available
    if systemctl --user status > /dev/null 2>&1 || systemctl --user list-units > /dev/null 2>&1; then
        "$BIN_DIR/deepspeed" install
        echo "Log: journalctl --user -u deepspeed -f"
        echo "     or: ~/.local/share/deepspeed/deepspeed.log"
    else
        echo "systemd user session not available."
        echo "Start manually: $BIN_DIR/deepspeed start &"
        echo "Or add to your session startup: $BIN_DIR/deepspeed start"
    fi
fi

echo
echo "=== Installation complete ==="
echo
echo "Commands:"
echo "  deepspeed status    — live system snapshot"
echo "  deepspeed config    — show active configuration"
echo "  deepspeed install   — reinstall daemon"
echo "  deepspeed uninstall — stop and remove daemon"
echo
echo "Note: Open a new terminal (or run: source ~/.zshrc) for 'deepspeed' to be in PATH."
