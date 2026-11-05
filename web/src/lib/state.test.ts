import { expect, test } from "bun:test";
import { stateOf } from "./state";
import type { AgentsItem } from "./api-types";

const agent = (over: Partial<AgentsItem>): AgentsItem => ({ id: "1", title: "", project: "", project_id: "", status: "idle", unseen: false, busy: false, questions: 0, used: "", place: "", edited: 0, awake: false, ...over });

test("a question comes first, then a failure, a stall, work", () => {
  expect(stateOf(agent({ busy: true, stalled: true, questions: 1 })).shape).toBe("wait");
  expect(stateOf(agent({ busy: true, stalled: true, failed: { times: 1 } })).shape).toBe("fail");
  expect(stateOf(agent({ busy: true, stalled: true })).shape).toBe("stall");
  expect(stateOf(agent({ busy: true })).shape).toBe("work");
  expect(stateOf(agent({ stalled: true })).shape).not.toBe("stall");
});
