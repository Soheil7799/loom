# loom

A light, keyboard-first home for coding agents, in GTK4. Each agent is a Claude Code session
driven over [ACP](https://agentclientprotocol.com) (`claude-agent-acp`), shown as a native chat
rather than a terminal; its commands land in a job tray, the files it writes open in a side
editor, and a scratch terminal is one key away.

    ./install.sh        # ~/.local/bin/loom, the desktop entry, and the adapter (npm)

Needs `gtk4`, `gtksourceview5`, `vte4` and Node. Uses your Claude Code login.

Keys: `Ctrl+/` in the app. Idle agents you aren't looking at stop after 15 minutes
(`LOOM_IDLE_MINUTES`) and resume when you open them.
