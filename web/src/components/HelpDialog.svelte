<script>
  import Modal from "./Modal.svelte";

  let { onclose } = $props();
  const KEYS = [
    ["j / k  ↑ / ↓", "move selection"],
    ["g / G  Home / End", "first / last"],
    ["1 – 4", "switch filter tab"],
    ["Enter", "toggle details"],
    ["a", "add downloads (or just paste or drop)"],
    ["p  r  Space", "pause / resume / toggle"],
    ["P  R", "pause all / resume all"],
    ["d  Delete", "remove"],
    ["X", "clear finished"],
    ["s  S", "speed limit / global speed limit"],
    ["c", "copy URL"],
    ["U", "update yt-dlp"],
    ["?  Esc", "this help / close"],
  ];
  const MOUSE = [
    ["click", "select a download"],
    ["double-click", "open details"],
    ["right-click  ⋯", "actions menu"],
    ["drop files or links", "add them"],
    ["click ↓ / ↑ speeds", "global speed limit"],
  ];
</script>

<Modal title="Keyboard & mouse" {onclose} dismissable>
  {#each [["Keyboard", KEYS], ["Mouse & touch", MOUSE]] as [heading, rows] (heading)}
    <h3 class="mt-1 mb-1 text-sm font-bold text-magenta">{heading}</h3>
    <dl class="mb-3 grid grid-cols-[minmax(8rem,auto)_1fr] gap-x-4 gap-y-1 text-sm">
      {#each rows as [k, v] (k)}
        <dt class="font-semibold text-blue">{k}</dt>
        <dd class="text-fg-dim">{v}</dd>
      {/each}
    </dl>
  {/each}
  {#snippet footer()}
    <button data-autofocus class="rounded-full bg-raised px-4 py-1.5 text-sm font-semibold hover:bg-line" onclick={onclose}>Close</button>
  {/snippet}
</Modal>
