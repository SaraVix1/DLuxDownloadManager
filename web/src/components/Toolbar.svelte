<script>
  import { can, selectedDownload } from "../lib/store.svelte.js";
  import { TOOLBAR, command } from "../lib/commands.js";

  // Re-evaluated when the selection or the state changes.
  const d = $derived(selectedDownload());
</script>

<nav class="flex flex-wrap items-center gap-2" aria-label="Actions">
  {#each TOOLBAR as b (b.cmd)}
    {@const enabled = can(b.cmd, d)}
    <button
      class="rounded-full px-4 py-1.5 text-sm font-semibold transition disabled:cursor-not-allowed
        {b.wide ? 'hidden md:inline-block' : ''}
        {b.primary ? 'bg-blue text-sunk hover:bg-cyan' : 'bg-raised text-fg enabled:hover:bg-line disabled:text-muted'}"
      disabled={!enabled}
      onclick={() => command(b.cmd)}
    >{b.label}</button>
  {/each}
  <button class="ml-auto rounded-full px-3 py-1.5 text-sm text-muted hover:bg-raised hover:text-fg" onclick={() => command("help")} aria-label="Keyboard shortcuts">?</button>
</nav>
