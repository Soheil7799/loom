# loom's logo

- **The mark:** a fox whose body is an l, its tail curling through the l's foot.
  `agile-l-dark.svg` / `agile-l-light.svg` (for dark and light backgrounds), and
  `agile-l-small-*.svg` with thicker lines for 16 to 32 px (the favicon uses it).
- **The wordmark:** "loom" in one line, the fox's head as the l, its tail at the end of the m.
  `wordmark-dark.svg` / `wordmark-light.svg`. For headers, the README and the GTK sidebar;
  below about 90 px wide (40 px tall), use the mark instead.
- **The alternative mark:** `clever-m-*.svg`, a fox head flowing into an m. Not in use.
- **The app icon** is `../dev.shirvani.Loom.svg` (the mark on a navy tile); the web page's
  favicon is `../../web/public/favicon.svg` (the small mark on the same tile).

**The PNGs** (`agile-l-*.png`, `wordmark-*.png`) are the SVGs rendered by librsvg, for the
GTK app (`src/logo.rs`), which can't count on an SVG loader. After editing an SVG, render
its PNG again:

```sh
for m in dark light; do
  rsvg-convert -w 256 agile-l-$m.svg -o agile-l-$m.png
  rsvg-convert -w 720 wordmark-$m.svg -o wordmark-$m.png
done
```

Colours are loom's palette, Nocturne: glacier blue `#c4e8ff` to `#4aa8ef` on dark,
`#3a9be0` to `#004f87` on light; gold `#ffe08a` to `#f5a524` (amber `#f9c440` to `#e48a00`
on light); navy tile `#132a44` to `#0a1729`.
