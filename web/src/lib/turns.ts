// A conversation's entries (`conversation/<agent>`) as turns: your message, then what the
// agent did, runs of finished tool calls folded into one line. A finished turn folds to your
// message and how it ended; the tube's marks (turn start, waits, failed) come from here.
import type { ConversationItem } from "./api-types";

export type Entry = ConversationItem;
/** A row of a turn: an entry, or a run of finished tool calls shown as one line. */
export type Row = { kind: "entry"; entry: Entry } | { kind: "run"; id: string; tools: Entry[] };
export type Turn = {
  id: string;
  /** your message that opened it (none for what came before the first one) */
  user?: Entry;
  rows: Row[];
  /** the agent is still at it */
  live: boolean;
  /** a question waits for you */
  waits: boolean;
  /** a step failed */
  failed: boolean;
  /** how it ended: the agent's last words */
  result?: string;
  /** the number of entries after your message */
  steps: number;
  /** how long it took, when ended */
  seconds?: number;
};

const finished = (e: Entry) => e.kind === "tool" && e.status === "done";

export function turns(entries: Entry[]): Turn[] {
  const out: Turn[] = [];
  let cur: Turn | undefined;
  for (const e of entries) {
    if (e.kind === "user") {
      cur = { id: e.turn_id ?? e.id, user: e, rows: [], live: false, waits: false, failed: false, steps: 0 };
      out.push(cur);
      continue;
    }
    if (!cur) {
      cur = { id: e.turn_id ?? "start", rows: [], live: false, waits: false, failed: false, steps: 0 };
      out.push(cur);
    }
    if (e.kind === "turn") {
      cur.seconds = e.seconds;
      continue;
    }
    cur.steps++;
    if (e.status === "failed") cur.failed = true;
    if (e.status === "awaiting_user") cur.waits = true;
    if (e.status === "pending" || e.status === "streaming" || e.status === "awaiting_user") cur.live = true;
    if (e.kind === "agent" && e.text) cur.result = e.text;
    const last = cur.rows[cur.rows.length - 1];
    if (finished(e)) {
      if (last?.kind === "run") last.tools.push(e);
      else cur.rows.push({ kind: "run", id: `run-${e.id}`, tools: [e] });
    } else {
      cur.rows.push({ kind: "entry", entry: e });
    }
  }
  // the last turn is live until the agent marks its end
  const end = out[out.length - 1];
  if (end && end.seconds === undefined && end.user) end.live = true;
  // a run of one is just the tool
  for (const t of out) t.rows = t.rows.map((r) => (r.kind === "run" && r.tools.length === 1 ? { kind: "entry", entry: r.tools[0] } : r));
  return out;
}

/** "Read 3 · Searched 1 · Ran 1": a run of tool calls, by what they did. */
export function summary(tools: Entry[]): string {
  const words: Record<string, string> = { read: "Read", search: "Searched", execute: "Ran", edit: "Edited", fetch: "Fetched", think: "Thought", delete: "Deleted", move: "Moved" };
  const counts = new Map<string, number>();
  for (const t of tools) {
    const w = words[t.tool ?? ""] ?? "Other";
    counts.set(w, (counts.get(w) ?? 0) + 1);
  }
  return [...counts].map(([w, n]) => `${w} ${n}`).join(" · ");
}
