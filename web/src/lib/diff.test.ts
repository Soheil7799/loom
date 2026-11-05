import { expect, test } from "bun:test";
import { hunks, lines, unified } from "./diff";

test("a changed line in the middle: kept, removed, added, kept", () => {
  const d = lines("a\nb\nc", "a\nB\nc");
  expect(d.map((l) => l.t + l.text)).toEqual([" a", "-b", "+B", " c"]);
});

test("added at the end, removed at the start", () => {
  expect(lines("x\ny", "y\nz").map((l) => l.t + l.text)).toEqual(["-x", " y", "+z"]);
});

test("hunks keep only changes and their context", () => {
  const d = lines("1\n2\n3\n4\n5\n6\n7", "1\n2\n3\nFOUR\n5\n6\n7");
  expect(hunks(d, 1).map((l) => l.t + l.text)).toEqual([" 3", "-4", "+FOUR", " 5"]);
});

test("a unified diff splits into its hunks, the file's header left out", () => {
  const diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-a\n+b\n c\n@@ -9 +9 @@\n-y\n+z\n";
  const hunks = unified(diff);
  expect(hunks.map((h) => h.head)).toEqual(["@@ -1,2 +1,2 @@", "@@ -9 +9 @@"]);
  expect(hunks[0].lines).toEqual([{ t: "-", text: "a" }, { t: "+", text: "b" }, { t: " ", text: "c" }]);
});
