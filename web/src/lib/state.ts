// An agent's state as the UI shows it: a shape (st-*) and a word, the same everywhere.
import type { AgentsItem } from "./api-types";

export type Shape = "wait" | "work" | "done" | "fail" | "rest" | "idle";

export function stateOf(a: AgentsItem): { shape: Shape; word: string } {
  if (a.questions > 0) return { shape: "wait", word: a.questions > 1 ? `waits · ${a.questions} questions` : "waits" };
  if (a.failed) return { shape: "fail", word: "failed" };
  if (a.busy) return { shape: "work", word: "working" };
  if (a.status === "suspended") return { shape: "rest", word: `resting since ${clock(a.used)}` };
  if (a.unseen) return { shape: "done", word: `done ${ago(a.used)}` };
  return { shape: "idle", word: ago(a.used) };
}

/** "16:02" from an RFC 3339 time; "" when there is none. */
export function clock(when?: string): string {
  if (!when) return "";
  const d = new Date(when);
  return isNaN(+d) ? "" : d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** "12 min ago", "2 h ago", "yesterday"… */
export function ago(when?: string, now = Date.now()): string {
  if (!when) return "";
  const s = (now - new Date(when).getTime()) / 1000;
  if (!(s >= 0)) return "";
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return s < 172800 ? "yesterday" : new Date(when).toLocaleDateString();
}
