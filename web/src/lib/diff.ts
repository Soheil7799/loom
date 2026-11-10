// A line diff for edit cards: the longest common run of lines kept, the rest - and +.
export type Line = { t: " " | "-" | "+"; text: string };

export function lines(before: string, after: string): Line[] {
  const a = before.split("\n"), b = after.split("\n");
  const n = a.length, m = b.length;
  // An O(n*m) table: past ~4M cells (two files of 2000 lines) the page would stall, so a
  // whole rewrite is shown as one: the old lines out, the new ones in.
  if (n * m > 4_000_000) return [...a.map((text) => ({ t: "-" as const, text })), ...b.map((text) => ({ t: "+" as const, text }))];
  const keep = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) keep[i][j] = a[i] === b[j] ? keep[i + 1][j + 1] + 1 : Math.max(keep[i + 1][j], keep[i][j + 1]);
  const out: Line[] = [];
  let i = 0, j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) { out.push({ t: " ", text: a[i] }); i++; j++; }
    else if (j < m && (i === n || keep[i][j + 1] > keep[i + 1][j])) out.push({ t: "+", text: b[j++] });
    else out.push({ t: "-", text: a[i++] });
  }
  return out;
}

/** Only the changed lines and `context` lines around them. */
export function hunks(all: Line[], context = 2): Line[] {
  const near = all.map((_, k) => all.slice(Math.max(0, k - context), k + context + 1).some((l) => l.t !== " "));
  return all.filter((_, k) => near[k]);
}

/** A unified diff (git's) as its hunks: each its `@@` line, its file and its lines, marked +, - or " ". */
export function unified(diff: string): { head: string; file: string; lines: Line[] }[] {
  const hunks: { head: string; file: string; lines: Line[] }[] = [];
  let file = "", inHunk = false;
  for (const raw of diff.split("\n")) {
    // a file's header (its "--- a/…", "+++ b/…") is no hunk's: lines count from its next @@
    if (raw.startsWith("diff --git ")) { file = raw.replace(/^diff --git a\/.* b\//, ""); inHunk = false; }
    else if (raw.startsWith("@@")) { hunks.push({ head: raw, file, lines: [] }); inHunk = true; }
    else if (inHunk && /^[ +-]/.test(raw)) hunks.at(-1)!.lines.push({ t: raw[0] as Line["t"], text: raw.slice(1) });
  }
  return hunks;
}
