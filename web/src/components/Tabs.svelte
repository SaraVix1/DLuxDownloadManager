<script>
  import { TABS, ui } from "../lib/store.svelte.js";

  const counts = $derived(ui.snap?.counts ?? { active: 0, waiting: 0, done: 0, errors: 0 });

  function select(id) {
    if (ui.tab !== id) {
      ui.tab = id;
      ui.selected = null;
    }
  }
</script>

<div class="flex gap-1 overflow-x-auto" role="tablist" aria-label="Filter downloads">
  {#each TABS as t, i (t.id)}
    <button
      role="tab"
      aria-selected={ui.tab === t.id}
      title="{t.label} ({i + 1})"
      class="shrink-0 rounded-full px-3.5 py-1 text-sm font-semibold transition
        {ui.tab === t.id ? 'bg-blue text-sunk' : 'text-fg-dim hover:bg-raised hover:text-fg'}"
      onclick={() => select(t.id)}
    >
      {t.label}
      <span class="tabular {ui.tab === t.id ? '' : t.id === 'errors' && counts[t.id] > 0 ? 'text-red' : 'text-muted'}">{counts[t.id]}</span>
    </button>
  {/each}
</div>
