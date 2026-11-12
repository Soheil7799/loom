#!/usr/bin/env bash
# Install loom for the current user: ~/.local/bin/loom, loomd (loom with no window, its user
# service installed, not enabled) and loom-remote (its remote bridge;
# from a release, its bin/; from a checkout, built first), its desktop entry and icon. loom drives Claude Code itself and
# keeps its own skills in ~/.config/loom/claude.
set -euo pipefail
cd "$(dirname "$0")"

bindir="$HOME/.local/bin"
apps="$HOME/.local/share/applications"
icons="$HOME/.local/share/icons/hicolor/scalable/apps"

if [[ -x bin/loom ]]; then
    built=bin
else
    # Capped parallelism: an all-core build overheats some laptops.
    cargo build --release --workspace -j "${JOBS:-6}"
    built=target/release
fi
install -Dm755 "$built/loom" "$bindir/loom"
install -Dm755 "$built/loom-remote" "$bindir/loom-remote"
install -Dm755 "$built/loomd" "$bindir/loomd"
install -Dm644 data/loomd.service "$HOME/.config/systemd/user/loomd.service"
install -Dm644 data/dev.shirvani.Loom.svg "$icons/dev.shirvani.Loom.svg"
mkdir -p "$apps"
sed "s|@BINDIR@|$bindir|" data/dev.shirvani.Loom.desktop.in > "$apps/dev.shirvani.Loom.desktop"
update-desktop-database -q "$apps" || true
gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

# Older versions put the loom skill in ~/.claude/skills; loom now brings its own.
if [[ -f "$HOME/.claude/skills/loom/SKILL.md" ]]; then
    echo "loom now keeps its skill in ~/.config/loom/claude: ~/.claude/skills/loom can be deleted."
fi
echo "Installed $bindir/loom"
