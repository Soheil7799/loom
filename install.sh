#!/usr/bin/env bash
# Install loom for the current user (building it first): ~/.local/bin/loom, its desktop
# entry, and the ACP adapter it drives (npm, into ~/.local/share/loom/adapter).
set -euo pipefail
cd "$(dirname "$0")"

bindir="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
adapter="$HOME/.local/share/loom/adapter"
# The adapter version loom was tested against.
acp_version="${LOOM_ACP_VERSION:-0.95.0}"

# Capped parallelism: an all-core build overheats this laptop.
cargo build --release -j "${JOBS:-6}"
install -Dm755 target/release/loom "$bindir/loom"
mkdir -p "$apps" "$adapter"
sed "s|@BINDIR@|$bindir|" data/dev.shirvani.Loom.desktop.in > "$apps/dev.shirvani.Loom.desktop"
update-desktop-database -q "$apps" || true

if [[ ! -x "$adapter/node_modules/.bin/claude-agent-acp" ]]; then
    (cd "$adapter" && { [[ -f package.json ]] || npm init -y >/dev/null; } && npm i --ignore-scripts "@agentclientprotocol/claude-agent-acp@$acp_version")
fi
echo "Installed $bindir/loom"
