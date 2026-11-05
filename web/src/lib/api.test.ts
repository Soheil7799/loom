import { expect, test } from "bun:test";
// api.ts reads the page's address when it loads: a bare one here
(globalThis as { location?: unknown }).location ??= { hash: "" };
(globalThis as { sessionStorage?: unknown }).sessionStorage ??= { getItem: () => null, setItem: () => {} };
const { Follow } = await import("./api");

test("each change gives a new items array (a UI keeping it redraws)", () => {
  const f = new Follow("bridges");
  const seen: unknown[] = [];
  f.watchers.add((x) => seen.push(x.items));
  f.apply({ sub: "s", seq: 1, kind: "page", items: [{ id: "remote", state: "online" }] } as never);
  f.apply({ sub: "s", seq: 2, kind: "synced" } as never);
  f.apply({ sub: "s", seq: 3, kind: "changes", ops: [{ op: "upsert", item: { id: "remote", state: "online", candidate: { id: "d", name: "phone" } } }] } as never);
  expect(seen.length).toBe(2);
  expect(seen[1]).not.toBe(seen[0]);
  expect((seen[1] as { candidate?: { name: string } }[])[0].candidate?.name).toBe("phone");
});
