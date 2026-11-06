<script lang="ts">
  // A job's terminal: its screen, then its output as it comes (`job/<id>`: the screen, then
  // bytes after it); a new run starts it over. What you type goes to the job (`job_input`);
  // the job takes the size of this pane (`job_resize`, one size for every client).
  import type { Terminal } from "@xterm/xterm";
  import "@xterm/xterm/css/xterm.css";
  import { command, follow, fromBase64, type Follow } from "./lib/api";

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
    let watching: ResizeObserver | undefined;
    Promise.all([import("@xterm/xterm"), import("@xterm/addon-fit")]).then(([{ Terminal }, { FitAddon }]) => {
      if (done) return;
      const look = getComputedStyle(el);
      term = new Terminal({
        cursorBlink: false,
        scrollback: 5000,
        fontFamily: look.fontFamily,
        fontSize: 13,
        // the theme's colours as the page has them resolved (light or dark)
        theme: { background: look.backgroundColor, foreground: look.color, cursor: look.color, selectionBackground: look.outlineColor },
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(el);
      // opened to be typed in (a password it waits for, say): the keys go to it, unless
      // something else has them (the command box, after Ctrl+J)
      if (!document.activeElement || document.activeElement === document.body) term.focus();
      stop = follow(name, draw(term));
      term.onData((text) => command("job_input", { job, text }).catch(() => {}));
      // the pane's size, told to the job once it settles
      let timer = 0;
      watching = new ResizeObserver(() => {
        clearTimeout(timer);
        timer = window.setTimeout(() => {
          const size = fit.proposeDimensions();
          if (!term || !size?.cols || !size?.rows || (size.cols === term.cols && size.rows === term.rows)) return;
          term.resize(size.cols, size.rows);
          command("job_resize", { job, rows: size.rows, cols: size.cols }).catch(() => {});
        }, 150);
      });
      watching.observe(el);
    });
    return () => { done = true; stop(); watching?.disconnect(); term?.dispose(); };
  });
</script>

<div class="term" bind:this={el}></div>
