import { expect, test } from "bun:test";
import { ago, stateOf } from "./state";
import type { AgentsItem } from "./api-types";

const agent = (over: Partial<AgentsItem>): AgentsItem => ({ id: "1", title: "", project: "", project_id: "", status: "idle", unseen: false, busy: false, questions: 0, used: "", place: "", edited: 0, awake: false, ...over });

test("a question comes first, then a failure, a stall, work", () => {
  expect(stateOf(agent({ busy: true, stalled: true, questions: 1 })).shape).toBe("wait");
  expect(stateOf(agent({ busy: true, stalled: true, failed: { times: 1 } })).shape).toBe("fail");
  expect(stateOf(agent({ busy: true, stalled: true })).shape).toBe("stall");
  expect(stateOf(agent({ busy: true })).shape).toBe("work");
  expect(stateOf(agent({ stalled: true })).shape).not.toBe("stall");
});

test("ago reads RFC 3339 and Unix seconds alike", () => {
  const now = Date.parse("2026-11-06T12:00:00Z");
  expect(ago("2026-11-06T11:58:00Z", now)).toBe("2 min ago");
  expect(ago(now / 1000 - 7200, now)).toBe("2 h ago");
  expect(ago(undefined, now)).toBe("");
  expect(ago(now / 1000 - 5 * 86400, now)).toBe(new Date(now - 5 * 86400000).toLocaleDateString());
});
