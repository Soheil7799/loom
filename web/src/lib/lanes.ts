// A commit graph's lanes, from commits newest first, each after all its children (git log
// --date-order): one row per commit, in a lane (a column). A line from a commit to a parent
// runs down a lane: its first parent's goes on in the commit's own lane (or joins a lane to
// its left already waiting for that parent), another parent's joins the lane waiting for it
// or takes a free one. Lanes waiting for one parent meet where it is (the leftmost), as git
// draws them. A parent not loaded yet: the line runs to the bottom.

/** From the commit in lane `from` at `row`, down lane `lane`, to the parent at row `to` in lane `at`. */
export type Edge = { from: number; row: number; lane: number; to: number | null; at: number };
export type Layout = { lanes: number[]; edges: Edge[]; width: number };

export function layout(commits: { hash: string | null; parents?: string[] }[]): Layout {
  // each lane: the hash it waits for, and the lines running down it to that commit
  const open: ({ hash: string; edges: Edge[] } | null)[] = [];
  const lanes: number[] = [];
  const edges: Edge[] = [];
  const free = (not = -1) => { const i = open.findIndex((l, k) => l === null && k !== not); return i === -1 ? open.length : i; };
  commits.forEach((c, row) => {
    const hash = c.hash ?? "";
    const waiting = open.flatMap((l, k) => (l?.hash === hash ? [k] : []));
    const lane = waiting.length ? waiting[0] : free();
    for (const k of waiting) {
      for (const e of open[k]!.edges) { e.to = row; e.at = lane; }
      open[k] = null;
    }
    lanes.push(lane);
    (c.parents ?? []).forEach((p, n) => {
      const there = open.findIndex((l) => l?.hash === p);
      const into = there !== -1 && (n > 0 || there < lane) ? there : n === 0 ? lane : free(lane);
      const edge: Edge = { from: lane, row, lane: into, to: null, at: into };
      edges.push(edge);
      if (open[into]?.hash === p) open[into]!.edges.push(edge);
      else open[into] = { hash: p, edges: [edge] };
    });
  });
  const width = Math.max(1, ...lanes.map((l) => l + 1), ...edges.map((e) => e.lane + 1));
  return { lanes, edges, width };
}

/** An edge as an SVG path, lanes `w` wide and rows `h` high; `rows`: where an open one ends. */
export function path(e: Edge, w: number, h: number, rows: number): string {
  const x = (lane: number) => lane * w + w / 2, y = (row: number) => row * h + h / 2;
  const [x1, y1, xl] = [x(e.from), y(e.row), x(e.lane)];
  if (e.to === null) return x1 === xl ? `M${x1} ${y1}V${rows * h}` : `M${x1} ${y1}C${x1} ${y1 + h / 2} ${xl} ${y1 + h / 2} ${xl} ${y1 + h}V${rows * h}`;
  const [x2, y2] = [x(e.at), y(e.to)];
  // a row away: one curve, from the commit to its parent
  if (e.to === e.row + 1 || (x1 === xl && xl === x2)) return x1 === x2 ? `M${x1} ${y1}V${y2}` : `M${x1} ${y1}C${x1} ${y1 + h / 2} ${x2} ${y2 - h / 2} ${x2} ${y2}`;
  // over to its lane within the first row, down it, over to the parent within the last
  let d = `M${x1} ${y1}`;
  if (x1 !== xl) d += `C${x1} ${y1 + h / 2} ${xl} ${y1 + h / 2} ${xl} ${y1 + h}`;
  d += `V${xl === x2 ? y2 : y2 - h}`;
  if (xl !== x2) d += `C${xl} ${y2 - h / 2} ${x2} ${y2 - h / 2} ${x2} ${y2}`;
  return d;
}
