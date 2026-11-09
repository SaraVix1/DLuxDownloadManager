<script>
  import { ui } from "../lib/store.svelte.js";
  import { command } from "../lib/commands.js";
  import { speed } from "../lib/format.js";

  const snap = $derived(ui.snap);
  const limit = (v) => (v ? ` ≤ ${v}` : "");
  const down = $derived(ui.limits && ui.limits.download ? ui.limits.download_text : "");
  const up = $derived(ui.limits && ui.limits.upload ? ui.limits.upload_text : "");
</script>

<header class="flex items-center gap-3 rounded-2xl border border-line bg-sunk/60 px-4 py-2.5">
  <div class="flex items-baseline gap-1.5 text-lg font-bold tracking-tight select-none">
    <span class="text-purple">◆</span>
    <span><span class="text-blue">d</span><span class="text-cyan">l</span><span class="text-teal">u</span><span class="text-magenta">x</span></span>
  </div>
  <p class="hidden text-sm italic text-muted lg:block">Download everything. Beautifully.</p>

  <div class="ml-auto flex items-center gap-3">
    {#if snap}
      <span class="hidden items-center gap-1.5 text-sm text-muted sm:flex">
        {#if snap.aria2.connected}
          <span class="size-2 rounded-full bg-green"></span>
          aria2 {snap.aria2.version} · {snap.aria2.origin}
        {:else}
          <span class="size-2 rounded-full bg-red"></span>
          <span class="text-red">aria2 offline</span>
        {/if}
      </span>
    {/if}
    <button
      class="group flex items-center gap-1.5 rounded-full text-sm font-semibold tabular"
      title="Global speed limit"
      onclick={() => command("globalLimit")}
    >
      <span class="rounded-full bg-green px-3 py-0.5 text-sunk transition group-hover:bg-teal">
        ↓ {speed(snap?.speed.download ?? 0)}{limit(down)}
      </span>
      <span class="rounded-full bg-magenta px-3 py-0.5 text-sunk transition group-hover:bg-purple">
        ↑ {speed(snap?.speed.upload ?? 0)}{limit(up)}
      </span>
    </button>
  </div>
</header>
