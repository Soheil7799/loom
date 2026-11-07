import { expect, test } from "bun:test";
import { layout, path } from "./lanes";

const c = (hash: string, ...parents: string[]) => ({ hash, parents });
const shape = (l: ReturnType<typeof layout>) => l.edges.map((e) => [e.from, e.row, e.lane, e.to, e.at]);

test("a straight history stays in one lane", () => {
  const l = layout([c("c", "b"), c("b", "a"), c("a")]);
  expect(l.lanes).toEqual([0, 0, 0]);
  expect(shape(l)).toEqual([[0, 0, 0, 1, 0], [0, 1, 0, 2, 0]]);
  expect(l.width).toBe(1);
});

test("a branch takes a lane of its own and meets main where they forked, as git draws it", () => {
  // m merges f into main (b); f and b both come from a
  const l = layout([c("m", "b", "f"), c("f", "a"), c("b", "a"), c("a")]);
  expect(l.lanes).toEqual([0, 1, 0, 0]);
  // the merge's second line goes over to f's lane; f's runs down its lane and back into a
  expect(shape(l)).toEqual([[0, 0, 0, 2, 0], [0, 0, 1, 1, 1], [1, 1, 1, 3, 0], [0, 2, 0, 3, 0]]);
  expect(l.width).toBe(2);
});

test("two branch tips get two lanes; a parent not loaded runs to the bottom", () => {
  const l = layout([c("x", "o"), c("y", "p")]);
  expect(l.lanes).toEqual([0, 1]);
  expect(l.edges.every((e) => e.to === null)).toBe(true);
  expect(path(l.edges[1], 14, 28, 2)).toBe("M21 42V56");
});

test("lines curve between lanes at their ends and run straight between", () => {
  expect(path({ from: 0, row: 0, lane: 0, to: 2, at: 0 }, 14, 28, 3)).toBe("M7 14V70");
  expect(path({ from: 0, row: 0, lane: 1, to: 1, at: 1 }, 14, 28, 3)).toBe("M7 14C7 28 21 28 21 42");
  expect(path({ from: 1, row: 1, lane: 1, to: 3, at: 0 }, 14, 28, 4)).toBe("M21 42V70C21 84 7 84 7 98");
});
