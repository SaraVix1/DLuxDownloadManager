<script>
  import { ui } from "../lib/store.svelte.js";
  import { eta, percent, progressColor, size, speed, statusText } from "../lib/format.js";

  /** @type {{ d: any, onopen: () => void }} */
  let { d, onopen } = $props();

  const selected = $derived(ui.selected === d.gid);
  const color = $derived(progressColor(d));
  const BAR = { green: "bg-green", yellow: "bg-yellow", teal: "bg-teal", red: "bg-red", muted: "bg-muted" };
  const TEXT = { green: "text-green", yellow: "text-yellow", teal: "text-teal", red: "text-red", muted: "text-muted" };
  const STATUS = { active: "text-green", waiting: "text-blue", complete: "text-teal", error: "text-red" };
  const TAGS = { torrent: "text-purple", magnet: "text-purple", video: "text-magenta", tool: "text-cyan" };

  function menu(e) {
    e.preventDefault();
    ui.selected = d.gid;
    ui.menu = { gid: d.gid, x: e.clientX, y: e.clientY };
  }

  function menuButton(e) {
    e.stopPropagation();
    const r = e.currentTarget.getBoundingClientRect();
    ui.selected = d.gid;
    ui.menu = { gid: d.gid, x: r.right, y: r.bottom };
  }
</script>

<!-- Keyboard: j/k, Enter and the shortcuts work on the selection (see App.svelte). -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_click_events_have_key_events -->
<li
  class="group relative cursor-default rounded-xl border px-4 py-3 transition select-none
    {selected ? 'border-line bg-raised' : 'border-transparent hover:bg-hover'}"
  aria-selected={selected}
  role="option"
  onclick={() => (ui.selected = d.gid)}
  ondblclick={onopen}
  oncontextmenu={menu}
>
  {#if selected}<span class="absolute top-3 bottom-3 left-0 w-1 rounded-r bg-blue"></span>{/if}
  <div class="flex items-baseline gap-2">
    <span class="min-w-0 truncate font-semibold">{d.name}</span>
    {#if d.kind !== "http"}<span class="shrink-0 text-xs font-medium {TAGS[d.kind]}">{d.kind}</span>{/if}
    <span class="ml-auto shrink-0 text-sm font-bold tabular {TEXT[color]}">{percent(d.progress)}</span>
    <button
      class="-mr-2 shrink-0 rounded-md px-1.5 text-muted opacity-60 hover:bg-line hover:text-fg sm:opacity-0 sm:group-hover:opacity-100 focus:opacity-100"
      aria-label="Actions for {d.name}"
      onclick={menuButton}>⋯</button>
  </div>
  <div class="mt-2 h-1 overflow-hidden rounded-full bg-line">
    <div class="h-full rounded-full transition-[width] duration-700 ease-out {BAR[color]}" style="width: {Math.min(d.progress, 1) * 100}%"></div>
  </div>
  <div class="mt-1.5 flex items-baseline gap-x-2 text-sm text-muted tabular">
    <span class="text-fg-dim">{size(d)}</span>
    {#if d.status === "active"}
      <span>·</span><span class="text-green">↓ {speed(d.download_speed)}</span>
      {#if d.upload_speed > 0}<span>·</span><span class="text-magenta">↑ {speed(d.upload_speed)}</span>{/if}
      {#if !d.seeding}<span class="hidden sm:inline">· ETA <span class="text-fg-dim">{eta(d.eta)}</span></span>{/if}
    {/if}
    <span class="ml-auto min-w-0 truncate {STATUS[d.status] ?? 'text-muted'}" title={statusText(d)}>{statusText(d)}</span>
  </div>
</li>
