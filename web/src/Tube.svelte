<script lang="ts">
  // The tube down the left of a conversation: one hose,
  // the tube and a swell per bead drawn through a "goo" filter so they melt into one shape;
  // beads above it, glossy and glowing, still. While the agent works, a live bead shuttles
  // between its latest entry and the tube's end (where the next one lands), on a spring.
  // Marks are the column's elements with data-bead="turn|wait|fail" (and data-bead-y: where
  // on the element the bead sits, px from its top).
  let { column, live }: { column: HTMLElement | undefined; live: boolean } = $props();

  type Bead = { kind: string; y: number };
  let beads = $state<Bead[]>([]);
  let height = $state(0);
  let run = $state<{ y0: number; y1: number } | null>(null);

  function measure() {
    if (!column) return;
    const top = column.getBoundingClientRect().top;
    height = column.scrollHeight;
    const found: Bead[] = [];
    for (const el of column.querySelectorAll<HTMLElement>("[data-bead]")) {
      if (!el.offsetParent) continue;
      found.push({ kind: el.dataset.bead!, y: el.getBoundingClientRect().top - top + Number(el.dataset.beadY ?? 12) });
    }
    beads = found;
    // the live run: from the latest entry to the tube's end, 40-160 px, so the spring's
    // overshoot (~8%) stays inside the tube
    const end = height - 24, y1 = end - 22;
    const last = column.querySelector<HTMLElement>("[data-latest]");
    const from = last ? last.getBoundingClientRect().top - top + 9 : y1 - 110;
    run = { y0: Math.min(Math.max(from, y1 - 160), y1 - 40), y1 };
  }

  $effect(() => {
    if (!column) return;
    let frame = 0;
    const later = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); };
    const resize = new ResizeObserver(later);
    resize.observe(column);
    const changes = new MutationObserver(later);
    changes.observe(column, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-bead", "open", "class"] });
    later();
    return () => { resize.disconnect(); changes.disconnect(); cancelAnimationFrame(frame); };
  });
</script>

<svg width="0" height="0" class="defs" aria-hidden="true"><filter id="loom-goo"><feGaussianBlur stdDeviation="2.2" /><feColorMatrix values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 18 -7" /></filter></svg>
<div class="hose" aria-hidden="true" style:height="{height}px">
  <span class="tube" style:height="{Math.max(0, height - 30)}px"></span>
  {#each beads as b, i (i)}<span class="swell" style:top="{b.y - 8}px"></span>{/each}
  {#if live && run}<span class="swell live" style:--y0="{run.y0 - 8}px" style:--y1="{run.y1 - 8}px"></span>{/if}
</div>
<div class="beads" aria-hidden="true" style:height="{height}px">
  {#each beads as b, i (i)}<span class="bead {b.kind}" style:top="{b.y}px"></span>{/each}
  {#if live && run}<span class="bead live" style:--y0="{run.y0}px" style:--y1="{run.y1}px"></span>{/if}
</div>

<style>
  .defs { position: absolute; }
  .hose, .beads { position: absolute; left: 0; top: 0; width: 20px; pointer-events: none; }
  .hose { filter: url(#loom-goo); opacity: 0.28; z-index: 0; }
  .tube { position: absolute; left: 7px; top: 6px; width: 6px; border-radius: 999px; background: var(--fg); }
  .swell { position: absolute; left: 2px; width: 16px; height: 16px; border-radius: 50%; background: var(--fg); }
  .beads { z-index: 2; }
  .bead { position: absolute; left: 10px; width: 0; height: 0; }
  .bead::after { content: ""; position: absolute; width: 8px; height: 8px; left: -4px; top: -4px; border-radius: 50%;
    background: radial-gradient(circle at 38% 35%, color-mix(in oklab, var(--c) 55%, white), var(--c) 62%);
    box-shadow: 0 0 9px 1px color-mix(in oklab, var(--c) 55%, transparent); }
  .bead.turn { --c: var(--accent); }
  .bead.wait { --c: var(--wait); }
  .bead.wait::after { border-radius: 1.5px; rotate: 45deg; width: 9px; height: 9px; left: -4.5px; top: -4.5px; }
  .bead.fail { --c: var(--err); }
  .bead.fail::after { border-radius: 2px; }
  .live { animation: move 3.6s infinite; }
  .bead.live { --c: var(--accent); }
  .bead.live::after { width: 10px; height: 10px; left: -5px; top: -5px; animation: stretch 3.6s linear infinite; }
  .bead.live::before { content: ""; position: absolute; bottom: 0; left: -3px; width: 6px; height: 36px; border-radius: 999px; transform-origin: center bottom;
    filter: blur(3px); background: linear-gradient(180deg, transparent, color-mix(in oklab, var(--c) 70%, transparent)); animation: trail 3.6s linear infinite; }
  .swell.live { animation: move 3.6s infinite, stretch 3.6s linear infinite; }
  @keyframes move {
    0% { top: var(--y0); animation-timing-function: linear(0, 0.018, 0.066, 0.136, 0.221, 0.314, 0.411, 0.507, 0.599, 0.685, 0.763, 0.833, 0.893, 0.943, 0.985, 1.018, 1.043, 1.062, 1.074, 1.081, 1.083, 1.083, 1.079, 1.074, 1.067, 1.059, 1.051, 1.043, 1.035, 1.028, 1.021, 1.015, 1.010, 1.006, 1.002, 0.999, 0.997, 0.995, 0.994, 0.993, 0.993, 0.993, 0.993, 0.994, 0.994, 0.995, 0.996, 0.996, 1); }
    50% { top: var(--y1); animation-timing-function: linear(0, 0.018, 0.066, 0.136, 0.221, 0.314, 0.411, 0.507, 0.599, 0.685, 0.763, 0.833, 0.893, 0.943, 0.985, 1.018, 1.043, 1.062, 1.074, 1.081, 1.083, 1.083, 1.079, 1.074, 1.067, 1.059, 1.051, 1.043, 1.035, 1.028, 1.021, 1.015, 1.010, 1.006, 1.002, 0.999, 0.997, 0.995, 0.994, 0.993, 0.993, 0.993, 0.993, 0.994, 0.994, 0.995, 0.996, 0.996, 1); }
    100% { top: var(--y0); }
  }
  /* along the tube (y) it stretches with its speed, across it (x) it squashes */
  @keyframes stretch { 0% { scale: 1 1; } 4% { scale: 0.86 1.55; } 9% { scale: 0.78 2.05; } 15% { scale: 0.88 1.5; } 20% { scale: 1.02 0.97; } 22.5% { scale: 1.12 0.86; } 29% { scale: 0.97 1.04; } 37.5% { scale: 1 1; } 50% { scale: 1 1; } 54% { scale: 0.86 1.55; } 59% { scale: 0.78 2.05; } 65% { scale: 0.88 1.5; } 70% { scale: 1.02 0.97; } 72.5% { scale: 1.12 0.86; } 79% { scale: 0.97 1.04; } 87.5% { scale: 1 1; } 100% { scale: 1 1; } }
  @keyframes trail { 0% { opacity: 0; scale: 1 1; } 4% { opacity: 0.7; scale: 1 1; } 9% { opacity: 1; scale: 1 1; } 16% { opacity: 0.6; scale: 1 1; } 22% { opacity: 0; scale: 1 1; } 50% { opacity: 0; scale: 1 -1; } 54% { opacity: 0.7; scale: 1 -1; } 59% { opacity: 1; scale: 1 -1; } 66% { opacity: 0.6; scale: 1 -1; } 72% { opacity: 0; scale: 1 -1; } 100% { opacity: 0; scale: 1 -1; } }
  @media (prefers-reduced-motion: reduce) {
    .live, .live::before, .live::after { animation: none !important; }
    .live { top: var(--y1) !important; }
    .bead.live::before { opacity: 0; }
  }
</style>
