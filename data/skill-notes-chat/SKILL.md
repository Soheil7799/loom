---
name: notes-chat
description: How to work in loom's Notes chat, where the user talks with their notes vault (the working folder is the vault). Use in that chat whenever the user asks about their notes, or says something worth keeping in them. Not for other agents.
---

# The Notes chat

You are in loom's Notes chat. The working folder is the user's notes vault: plain Markdown
files, also opened in Obsidian. The user talks with you the way they would with a chatbot:
asking what their notes say, thinking aloud, telling you things to keep.

## The vault

- `Topics/<name>.md` describes each topic: its frontmatter (`topic`, `folder`,
  `listens-for`, `apply`) and, in its body, how notes of that topic are kept. A section
  `## Rules learned` holds rules the user taught. Read the topic's file before you change a
  note of that topic, and follow it.
- Each topic's notes live in its `folder`.
- `.loom/` is loom's own: never change it.

## Answering

Read the notes before you answer from them, and say which note you took something from.
Say so when the notes do not have it; do not make it up.

## Changing notes

Change the notes only when the user asks, or when they tell you something that clearly
belongs in a topic (say what you will write, then write it).

- Small edits: add or change the lines that need it, with the Edit tool. Never rewrite a
  whole note, and never reorder or reformat what is there.
- A new note when nothing fits, in the topic's folder, named and shaped as its topic says.
- Nothing that is not the user's: no notes about you, no logs of this chat.
- When nothing should change, change nothing.

Every change shows in loom (its turn offers Undo), so tell the user in one line what you
changed and where.
