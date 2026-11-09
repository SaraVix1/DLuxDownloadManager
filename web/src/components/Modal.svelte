<script>
  import { fade, scale } from "svelte/transition";

  /** @type {{ title: string, onclose: () => void, dismissable?: boolean, wide?: boolean, children: any, footer?: any }} */
  let { title, onclose, dismissable = false, wide = false, children, footer } = $props();
  let box = $state();

  // Focus the first field (or button) when opened; restore focus when closed.
  $effect(() => {
    const before = document.activeElement;
    queueMicrotask(() => {
      box?.querySelector("[data-autofocus], input, textarea, select, button")?.focus();
    });
    return () => before?.focus?.();
  });

  function keydown(e) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    } else if (e.key === "Tab") {
      // Keep Tab inside the dialog.
      const items = [...box.querySelectorAll("button, input, textarea, select, [tabindex]:not([tabindex='-1'])")].filter((x) => !x.disabled);
      if (!items.length) return;
      const [first, last] = [items[0], items.at(-1)];
      if (e.shiftKey && document.activeElement === first) {
        last.focus();
        e.preventDefault();
      } else if (!e.shiftKey && document.activeElement === last) {
        first.focus();
        e.preventDefault();
      }
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="fixed inset-0 z-50 grid place-items-center bg-black/55 p-3 backdrop-blur-[2px]" transition:fade={{ duration: 120 }} onclick={(e) => dismissable && e.target === e.currentTarget && onclose()}>
  <div
    bind:this={box}
    role="dialog"
    aria-modal="true"
    aria-label={title}
    tabindex="-1"
    onkeydown={keydown}
    transition:scale={{ start: 0.97, duration: 140 }}
    class="flex max-h-[calc(100dvh-1.5rem)] w-full flex-col rounded-2xl border border-line-focus bg-sunk shadow-2xl shadow-black/60 {wide ? 'max-w-2xl' : 'max-w-lg'}"
  >
    <h2 class="px-5 pt-4 pb-2 font-bold text-blue">{title}</h2>
    <div class="min-h-0 overflow-y-auto px-5 pb-2">{@render children()}</div>
    {#if footer}<div class="flex flex-wrap items-center justify-end gap-2 px-5 pt-2 pb-4">{@render footer()}</div>{/if}
  </div>
</div>
