<script lang="ts">
  // A line icon (Lucide style, 2 px at 24 px), drawn in the text colour.
  let { name, size = 16, label }: { name: keyof typeof paths; size?: number; label?: string } = $props();
</script>

<script lang="ts" module>
  export const paths = {
    check: "M20 6 9 17l-5-5",
    x: "M18 6 6 18M6 6l12 12",
    "chev-r": "m9 18 6-6-6-6",
    "chev-d": "m6 9 6 6 6-6",
    plus: "M5 12h14M12 5v14",
    play: "M6 4l13 8-13 8z",
    stop: "M7 7h10v10H7z",
    send: "M12 19V5M5 12l7-7 7 7",
    terminal: "m4 17 6-6-6-6M12 19h8",
    grid: "M3 3h7v7H3zM14 3h7v7h-7zM14 14h7v7h-7zM3 14h7v7H3z",
    inbox: "M22 12h-6l-2 3h-4l-2-3H2M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z",
    help: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3M12 17h.01",
    undo: "M9 14 4 9l5-5M4 9h10.5a5.5 5.5 0 0 1 0 11H11",
    diff: "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7ZM9 10h6M12 13V7M9 17h6",
    pencil: "M21.17 6.81a1 1 0 0 0-3.99-3.99L3.84 16.17a2 2 0 0 0-.5.83l-1.32 4.35a.5.5 0 0 0 .62.62l4.35-1.32a2 2 0 0 0 .83-.5z",
    back: "m12 19-7-7 7-7M19 12H5",
  } as const;
</script>

<svg class="ic" width={size} height={size} viewBox="0 0 24 24" aria-hidden={label ? undefined : "true"} role={label ? "img" : undefined} aria-label={label}>
  <path d={paths[name]} />
</svg>

<style>
  .ic { flex: none; fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; vertical-align: -0.15em; }
</style>
