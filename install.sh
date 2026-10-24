#!/usr/bin/env bash
# Install loom for the current user (building it first): ~/.local/bin/loom, its desktop
# entry and icon, the ACP adapter it drives (npm, into ~/.local/share/loom/adapter), and the
# `loom` skill that teaches Claude Code its tools (~/.claude/skills/loom).
# LOOM_SKILL=0 skips the skill.
set -euo pipefail
cd "$(dirname "$0")"

bindir="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/scalable/apps"
adapter="$HOME/.local/share/loom/adapter"
skills="$HOME/.claude/skills/loom"
# The adapter version loom was tested against.
acp_version="${LOOM_ACP_VERSION:-0.95.0}"

# Capped parallelism: an all-core build overheats this laptop.
cargo build --release -j "${JOBS:-6}"
install -Dm755 target/release/loom "$bindir/loom"
install -Dm644 data/dev.shirvani.Loom.svg "$icons/dev.shirvani.Loom.svg"
mkdir -p "$apps" "$adapter"
sed "s|@BINDIR@|$bindir|" data/dev.shirvani.Loom.desktop.in > "$apps/dev.shirvani.Loom.desktop"
update-desktop-database -q "$apps" || true
gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

installed="$(node -p "require('$adapter/node_modules/@agentclientprotocol/claude-agent-acp/package.json').version" 2>/dev/null || true)"
if [[ "$installed" != "$acp_version" ]]; then
    (cd "$adapter" && { [[ -f package.json ]] || npm init -y >/dev/null; } && npm i --ignore-scripts "@agentclientprotocol/claude-agent-acp@$acp_version")
fi

if [[ "${LOOM_SKILL:-1}" == 1 ]]; then
    install -Dm644 data/skill/SKILL.md "$skills/SKILL.md"
fi
echo "Installed $bindir/loom"
