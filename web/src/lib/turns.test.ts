import { expect, test } from "bun:test";
import { summary, turns, type Entry } from "./turns";

const e = (id: string, kind: string, status = "done", more: Partial<Entry> = {}): Entry => ({ id, kind, status, turn_id: more.turn_id ?? "t1", ...more });

test("a finished turn: your message, a run of tools folded, the result, its length", () => {
  const ts = turns([
    e("e1", "user", "done", { text: "fix it" }),
    e("e2", "tool", "done", { tool: "read" }),
    e("e3", "tool", "done", { tool: "read" }),
    e("e4", "tool", "done", { tool: "search" }),
    e("e5", "agent", "done", { text: "Fixed." }),
    e("e6", "turn", "done", { seconds: 12 }),
  ]);
  expect(ts.length).toBe(1);
  const [t] = ts;
  expect(t.user?.text).toBe("fix it");
  expect(t.live).toBe(false);
  expect(t.result).toBe("Fixed.");
  expect(t.seconds).toBe(12);
  expect(t.steps).toBe(4);
  expect(t.rows.map((r) => r.kind)).toEqual(["run", "entry"]);
  expect(summary((t.rows[0] as { tools: Entry[] }).tools)).toBe("Read 2 · Searched 1");
});

test("the last turn is live; a question waits; a failure is kept apart from runs", () => {
  const ts = turns([
    e("e1", "user", "done", { turn_id: "t1" }),
    e("e2", "agent", "done", { turn_id: "t1", text: "ok" }),
    e("e3", "turn", "done", { turn_id: "t1", seconds: 3 }),
    e("e4", "user", "done", { turn_id: "t2" }),
    e("e5", "tool", "done", { turn_id: "t2", tool: "read" }),
    e("e6", "tool", "failed", { turn_id: "t2", tool: "execute" }),
    e("e7", "tool", "done", { turn_id: "t2", tool: "read" }),
    e("e8", "question", "awaiting_user", { turn_id: "t2" }),
  ]);
  expect(ts.map((t) => t.live)).toEqual([false, true]);
  const t2 = ts[1];
  expect(t2.waits).toBe(true);
  expect(t2.failed).toBe(true);
  // one-tool "runs" are plain entries; the failed tool breaks the run
  expect(t2.rows.map((r) => (r.kind === "entry" ? r.entry.id : "run"))).toEqual(["e5", "e6", "e7", "e8"]);
});

test("entries before any message of yours form their own turn", () => {
  const ts = turns([e("e1", "notice", "done", { turn_id: undefined, text: "resumed" }), e("e2", "user", "done")]);
  expect(ts.length).toBe(2);
  expect(ts[0].user).toBeUndefined();
});
