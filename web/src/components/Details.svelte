<script>
  import { selectedDownload, ui } from "../lib/store.svelte.js";
  import { bytes, eta, percent, size, speed, statusText } from "../lib/format.js";

  let { onclose = null } = $props();
  const d = $derived(selectedDownload());
  const VIA = { video: "yt-dlp (with aria2c)", tool: "dlux tool installer" };
</script>

<aside class="flex min-h-0 flex-col rounded-2xl border border-line" aria-label="Details">
  <div class="flex items-center px-4 pt-3 pb-1">
    <h2 class="text-sm font-bold text-fg-dim">Details</h2>
    {#if onclose}
      <button class="ml-auto rounded-md px-2 text-muted hover:bg-raised hover:text-fg" aria-label="Close details" onclick={onclose}>✕</button>
    {/if}
  </div>
  {#if !d}
    <p class="grid flex-1 place-items-center px-4 py-10 text-center text-sm text-muted">Select a download to see its details</p>
  {:else}
    <div class="min-h-0 overflow-y-auto px-4 pb-4">
      <p class="mb-2 font-semibold break-words">{d.name}</p>
      <dl class="grid grid-cols-[7.5rem_1fr] gap-x-2 gap-y-1 text-sm">
        <dt class="text-muted">Status</dt><dd class="break-words">{statusText(d)}</dd>
        <dt class="text-muted">Progress</dt><dd class="tabular">{percent(d.progress)} · {size(d)}</dd>
        <dt class="text-muted">Speed</dt><dd class="tabular">↓ {speed(d.download_speed)} &nbsp;↑ {speed(d.upload_speed)}</dd>
        {#if d.status === "active" && !d.seeding}<dt class="text-muted">ETA</dt><dd class="tabular">{eta(d.eta)}</dd>{/if}
        {#if VIA[d.kind]}
          <dt class="text-muted">Via</dt><dd>{VIA[d.kind]}</dd>
        {:else}
          <dt class="text-muted">Connections</dt><dd class="tabular">{d.connections}</dd>
        {/if}
        {#if d.kind === "torrent" || d.kind === "magnet"}
          <dt class="text-muted">Seeds</dt><dd class="tabular">{d.seeders}{d.seeding ? " (seeding)" : ""}</dd>
        {/if}
        <dt class="text-muted">Directory</dt><dd class="font-mono text-xs break-all">{d.dir}</dd>
        <dt class="text-muted">GID</dt><dd class="font-mono text-xs">{d.gid}</dd>
        {#if d.url}<dt class="text-muted">URL</dt><dd class="font-mono text-xs break-all">{d.url}</dd>{/if}
      </dl>
      {#if d.files.length}
        <h3 class="mt-4 mb-1 text-sm font-bold text-blue">Files ({d.files.length})</h3>
        <ul class="space-y-0.5 text-sm">
          {#each d.files.slice(0, 200) as f (f.path)}
            <li class="flex gap-2 {f.selected ? 'text-fg-dim' : 'text-muted line-through'}">
              <span class="w-12 shrink-0 text-right text-muted tabular">{f.length ? Math.round((f.completed / f.length) * 100) : 0}%</span>
              <span class="min-w-0 flex-1 truncate" title={f.path}>{f.path.split(/[\\/]/).pop()}</span>
              <span class="shrink-0 text-muted tabular">{bytes(f.length)}</span>
            </li>
          {/each}
        </ul>
        {#if d.files.length > 200}<p class="mt-1 text-sm text-muted">… and {d.files.length - 200} more</p>{/if}
      {/if}
    </div>
  {/if}
</aside>
