<script>
  import Modal from "./Modal.svelte";
  import { api } from "../lib/api.js";
  import { speed } from "../lib/format.js";
  import { toast, ui } from "../lib/store.svelte.js";

  let { gid = null, name, onclose } = $props();
  let down = $state("");
  let up = $state("");
  let loaded = $state(false);
  let error = $state("");

  $effect(() => {
    api.limits(gid).then(
      (l) => {
        // Keep anything typed while loading.
        if (!down) down = l.download_text;
        if (!up) up = l.upload_text;
        loaded = true;
      },
      (e) => (error = e.message),
    );
  });

  // Rough preview; the server has the final word.
  function preview(v) {
    const m = v.trim().toLowerCase().replace(/\/s$/, "").match(/^(\d+(?:\.\d+)?)\s*([kmg]?)(?:i?b)?$/);
    if (!v.trim() || v.trim() === "0") return "= unlimited";
    if (!m) return "?";
    const mult = { "": 1, k: 1024, m: 1024 ** 2, g: 1024 ** 3 }[m[2]];
    const b = Math.round(parseFloat(m[1]) * mult);
    return b === 0 ? "= unlimited" : `= ${speed(b)}`;
  }

  async function apply() {
    error = "";
    try {
      const l = await api.setLimits(gid, down, up);
      if (!gid) ui.limits = l;
      toast(`${gid ? "Limit" : "Global limit"}: ↓ ${l.download ? speed(l.download) : "unlimited"}, ↑ ${l.upload ? speed(l.upload) : "unlimited"}`);
      onclose();
    } catch (e) {
      error = e.message;
    }
  }

  const enter = (e) => e.key === "Enter" && apply();
</script>

<Modal title={gid ? "Speed limit" : "Speed limit · all downloads"} {onclose}>
  <p class="mb-3 truncate font-semibold" title={name}>{name}</p>
  <div class="grid grid-cols-[6rem_9rem_1fr] items-center gap-x-3 gap-y-2 text-sm">
    <label for="lim-down" class="font-semibold text-fg-dim">Download</label>
    <input id="lim-down" data-autofocus bind:value={down} onkeydown={enter} placeholder={loaded ? "0" : "loading…"} class="rounded-lg border border-line bg-base px-2.5 py-1.5 font-mono tabular focus:border-line-focus focus:outline-none" />
    <span class="text-muted tabular">{loaded || down ? preview(down) : ""}</span>
    <label for="lim-up" class="font-semibold text-fg-dim">Upload</label>
    <input id="lim-up" bind:value={up} onkeydown={enter} placeholder={loaded ? "0" : "loading…"} class="rounded-lg border border-line bg-base px-2.5 py-1.5 font-mono tabular focus:border-line-focus focus:outline-none" />
    <span class="text-muted tabular">{loaded || up ? preview(up) : ""}</span>
  </div>
  <p class="mt-3 text-sm {error ? 'text-red' : 'text-muted'}">{error ? `✗ ${error}` : "e.g. 500K, 2M, 1.5M · 0 = unlimited"}</p>
  {#snippet footer()}
    <button class="rounded-full bg-raised px-4 py-1.5 text-sm font-semibold hover:bg-line" onclick={onclose}>Cancel</button>
    <button class="rounded-full bg-blue px-4 py-1.5 text-sm font-semibold text-sunk hover:bg-cyan" onclick={apply}>Apply</button>
  {/snippet}
</Modal>
