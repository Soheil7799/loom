#!/usr/bin/env python3
"""A stand-in for Claude Code over ACP (newline-delimited JSON-RPC on stdin/stdout), playing
two turns for loomd's tests:
- "background": says "started", leaves a background subagent running and ends its turn; the
  subagent ends 1.5 s later and the agent goes on by itself: "all finished".
- "server": says "serving", leaves a background command (a dev server) running, ends its turn.
- "plan": says its plan (two steps).
Anything else is answered with an empty result."""
import json, sys, threading, time

out = threading.Lock()


def send(message):
    with out:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def update(u):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": u}})


def say(text):
    update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}})


def later():
    time.sleep(1.5)
    update({"sessionUpdate": "subagent_update", "sessionId": "sub1", "state": {"state": "idle"}})
    time.sleep(0.5)
    say("all finished")


for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg or "method" not in msg:
        continue
    method, rid = msg["method"], msg["id"]
    if method == "initialize":
        result = {"protocolVersion": 1, "agentCapabilities": {}}
    elif method == "session/new":
        result = {"sessionId": "s1"}
    elif method == "session/prompt":
        prompt = " ".join(b.get("text", "") for b in msg["params"].get("prompt", []))
        if "background" in prompt:
            say("started")
            update({"sessionUpdate": "subagent_update", "sessionId": "sub1", "description": "helper", "state": {"state": "running"}})
            threading.Thread(target=later, daemon=True).start()
        elif "plan" in prompt:
            update({"sessionUpdate": "plan", "entries": [{"content": "Read it", "status": "completed", "priority": "high"}, {"content": "Fix it", "status": "in_progress", "priority": "high"}]})
            say("planned")
        elif "bus" in prompt:
            # the session bus, as `systemd-run --user` would reach it (the strict sandbox shuts it)
            import os, socket
            path = f"/run/user/{os.getuid()}/bus"
            try:
                socket.socket(socket.AF_UNIX).connect(path)
                say("bus reachable")
            except PermissionError:
                say("bus blocked")
            except OSError:
                say("no bus")
        elif "write" in prompt:
            # as a shell command would (`printf > made.txt`): no diff for loom to see; after a
            # while, as an agent thinks first (loom reads git's status as the turn begins)
            time.sleep(1)
            with open("made.txt", "w") as f:
                f.write("made\n")
            say("wrote")
        elif "server" in prompt:
            say("serving")
            update({"sessionUpdate": "background_tasks", "tasks": [{"taskId": "t1", "toolCallId": "c1", "taskType": "local_bash", "description": "dev server"}]})
        result = {"stopReason": "end_turn"}
    else:
        result = {}
    send({"jsonrpc": "2.0", "id": rid, "result": result})
