<script>
  import { can, find, ui } from "../lib/store.svelte.js";
  import { MENU, command } from "../lib/commands.js";

  const d = $derived(ui.menu ? find(ui.menu.gid) : undefined);
  let el = $state();
  let pos = $state({ x: 0, y: 0 });

  // Keep the menu on screen.
  $effect(() => {
    if (!ui.menu || !el) return;
    const r = el.getBoundingClientRect();
    pos = {
      x: Math.max(4, Math.min(ui.menu.x, innerWidth - r.width - 4)),
      y: Math.max(4, Math.min(ui.menu.y, innerHeight - r.height - 4)),
    };
    el.querySelector("button:not(:disabled)")?.focus();
  });

  function keydown(e) {
    const items = [...el.querySelectorAll("button:not(:disabled)")];
    const i = items.indexOf(document.activeElement);
    if (e.key === "ArrowDown" || e.key === "j") items[(i + 1) % items.length]?.focus();
    else if (e.key === "ArrowUp" || e.key === "k") items[(i - 1 + items.length) % items.length]?.focus();
    else if (e.key === "Escape") ui.menu = null;
    else {
      const item = MENU.find((m) => m.key === e.key);
      if (item) command(item.cmd, d);
      else return;
    }
    e.preventDefault();
    e.stopPropagation();
  }
</script>

{#if ui.menu && d}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="fixed inset-0 z-40" onclick={() => (ui.menu = null)} oncontextmenu={(e) => { e.preventDefault(); ui.menu = null; }}></div>
  <div
    bind:this={el}
    role="menu"
    tabindex="-1"
    aria-label="Actions for {d.name}"
    class="fixed z-50 min-w-48 rounded-xl border border-line-focus bg-sunk p-1 shadow-2xl shadow-black/50"
    style="left: {pos.x}px; top: {pos.y}px"
    onkeydown={keydown}
  >
    {#each MENU as m (m.cmd)}
      <button
        role="menuitem"
        class="flex w-full items-center rounded-lg px-3 py-1.5 text-left text-sm enabled:hover:bg-raised enabled:hover:text-blue focus:bg-raised focus:text-blue focus:outline-none disabled:text-muted"
        disabled={!can(m.cmd, d)}
        onclick={() => command(m.cmd, d)}
      >
        {m.label}<kbd class="ml-auto pl-6 font-sans text-xs text-muted">{m.key}</kbd>
      </button>
    {/each}
  </div>
{/if}
