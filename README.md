# loom

A light, keyboard-first home for coding agents, in GTK4. Each agent is a Claude Code session
driven over [ACP](https://agentclientprotocol.com) (`claude-agent-acp`), shown as a native chat
rather than a terminal. Agents are grouped by project; a project may hold several git repos,
and each agent shows the one it works in. Commands run in a job tray, files open in a side
editor, and a scratch terminal is one key away.

Agents get loom's own MCP server (`loom mcp`): they run long commands as named jobs in the
tray, wait on them, and open files for you. The `loom` skill tells them when.

    ./install.sh        # ~/.local/bin/loom, desktop entry, adapter (npm), the loom skill

Needs `gtk4`, `gtksourceview5`, `vte4` and Node. Uses your Claude Code login.

`Ctrl+Shift+P` lists everything; `Ctrl+/` the keys; `Ctrl+B` is herdr's prefix.
