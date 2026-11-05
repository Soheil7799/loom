<script lang="ts">
  // A job's terminal, read-only: its screen at the job's own size, then its output as it
  // comes (`job/<id>`: the screen, then bytes after it). A new run starts it over.
  import type { Terminal } from "@xterm/xterm";
  import "@xterm/xterm/css/xterm.css";
  import { follow, fromBase64, type Follow } from "./lib/api";

  let { job }: { job: string } = $props();
  let el: HTMLElement;

  function draw(term: Terminal) {
    let screen: string | undefined;
    let written = 0;
    return (f: Follow) => {
      const item = f.items[0];
      const tail: Uint8Array = item?.tail ?? new Uint8Array();
      if (!item || item.screen !== screen || tail.length < written) {
        term.reset();
        screen = item?.screen;
        written = 0;
        if (!item) return;
        if (item.cols && item.rows) term.resize(item.cols, item.rows);
        term.write(fromBase64(item.screen));
      }
      if (tail.length > written) term.write(tail.slice(written));
      written = tail.length;
    };
  }

  $effect(() => {
    const name = `job/${job}`;
    let done = false;
    let stop = () => {};
    let term: Terminal | undefined;
    // xterm.js loads with the first terminal opened, not with the page
    import("@xterm/xterm").then(({ Terminal }) => {
      if (done) return;
      const look = getComputedStyle(el);
      term = new Terminal({
        disableStdin: true,
        cursorBlink: false,
        scrollback: 5000,
        fontFamily: look.fontFamily,
        fontSize: 13,
        // the theme's colours as the page has them resolved (light or dark)
        theme: { background: look.backgroundColor, foreground: look.color, cursor: look.color, selectionBackground: look.outlineColor },
      });
      term.open(el);
      stop = follow(name, draw(term));
    });
    return () => { done = true; stop(); term?.dispose(); };
  });
</script>

<div class="term" bind:this={el}></div>
