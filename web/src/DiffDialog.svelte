<script lang="ts">
  // A diff in its own window, over the rest, as the window opens one (Esc closes it): a
  // changed file's, or a commit's (who, when, its message first). Given `undo`, each hunk
  // can be gone back on; given `comment`, a line gets a note for the next message.
  import { unified } from "./lib/diff";

  let { title, text, about = "", undo, undoAll, comment, close }: { title: string; text: string; about?: string; undo?: (n: number, hunk: string) => void; undoAll?: () => void; comment?: (line: string) => void; close: () => void } = $props();
  const hunks = $derived(unified(text));
</script>

<div class="scrim top" role="presentation" onclick={close}></div>
<!-- svelte-ignore a11y_autofocus -->
<div class="diff-window" role="dialog" autofocus aria-modal="true" aria-label={title} tabindex="-1" onkeydown={(e) => e.key === "Escape" && close()}>
  <div class="ebar"><code>{title}</code><span class="grow"></span>{#if undoAll}<button class="btn small deny" onclick={undoAll}>Throw away all of it</button>{/if}<button class="btn small" onclick={close}>Close</button></div>
  <div class="diffs">
    {#if about}<pre class="about">{about}</pre>{/if}
    {#each hunks as h, i (i)}
      <div class="diff">
        <div class="f"><span>{h.file && about ? `${h.file}  ${h.head}` : h.head}</span>{#if undo}<button class="btn small" title="Undo only this change" onclick={() => undo!(i, [h.head, ...h.lines.map((l) => l.t + l.text)].join("\n"))}>Undo</button>{/if}</div>
        <pre>{#each h.lines as l, k (k)}{#if comment}<button class="dl {l.t === '+' ? 'add' : l.t === '-' ? 'del' : ''}" title="A note on this line, for your next message" onclick={() => comment!(l.t + l.text)}>{l.t}{l.text}</button>{:else}<span class={l.t === "+" ? "add" : l.t === "-" ? "del" : ""}>{l.t}{l.text}
</span>{/if}{/each}</pre>
      </div>
    {:else}<p class="quiet">{text || "No changes to show."}</p>{/each}
  </div>
</div>
