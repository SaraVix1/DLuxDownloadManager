<script>
  import { ui } from "../lib/store.svelte.js";
  import { fly } from "svelte/transition";
</script>

<div class="pointer-events-none fixed right-4 bottom-4 z-[60] flex max-w-[min(28rem,calc(100vw-2rem))] flex-col gap-2" aria-live="polite">
  {#each ui.toasts as t (t.id)}
    <div
      transition:fly={{ y: 12, duration: 180 }}
      class="pointer-events-auto rounded-xl border px-4 py-2.5 text-sm shadow-xl shadow-black/40
        {t.error ? 'border-red/40 bg-sunk text-red' : 'border-line bg-sunk text-green'}"
      role={t.error ? "alert" : "status"}
    >
      {t.error ? "✗" : "✓"} <span class={t.error ? "" : "text-fg"}>{t.text}</span>
    </div>
  {/each}
</div>
