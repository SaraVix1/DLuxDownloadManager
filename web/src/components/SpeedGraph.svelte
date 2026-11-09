<script>
  import { ui } from "../lib/store.svelte.js";
  import { niceCeiling, speed } from "../lib/format.js";

  const CAP = 60;
  const W = 600;
  const H = 120;

  const history = $derived(ui.snap?.history ?? []);
  const peak = $derived(Math.max(0, ...history.map(([d, u]) => Math.max(d, u))));
  const top = $derived(niceCeiling(Math.max(peak, 64 * 1024)));

  // Points right-aligned so the newest sample is at the right edge.
  function path(index, area) {
    if (history.length === 0) return "";
    const start = CAP - history.length;
    const pts = history.map((s, i) => {
      const x = ((start + i) / (CAP - 1)) * W;
      const y = H - (s[index] / top) * (H - 4) - 2;
      return [x, y];
    });
    if (pts.length === 1) pts.unshift([pts[0][0] - 1, pts[0][1]]);
    const line = pts.map(([x, y], i) => `${i ? "L" : "M"}${x.toFixed(1)},${y.toFixed(1)}`).join(" ");
    return area ? `${line} L${pts.at(-1)[0]},${H} L${pts[0][0]},${H} Z` : line;
  }
</script>

<section class="rounded-2xl border border-line px-4 pt-2 pb-3" aria-label="Speed, last 60 seconds">
  <div class="mb-1 flex items-center gap-3 text-xs">
    <span class="font-semibold text-fg-dim">Speed</span>
    <span class="text-muted">last 60s</span>
    <span class="ml-auto flex items-center gap-3 text-muted">
      <span class="flex items-center gap-1"><span class="h-0.5 w-3 rounded bg-green"></span>down</span>
      <span class="flex items-center gap-1"><span class="h-0.5 w-3 rounded bg-magenta"></span>up</span>
      <span class="tabular">peak {speed(peak)}</span>
    </span>
  </div>
  <div class="relative">
    <svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" class="h-24 w-full sm:h-28" role="img" aria-label="Download and upload speed">
      <defs>
        <linearGradient id="g-down" x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stop-color="var(--color-green)" stop-opacity="0.28" />
          <stop offset="1" stop-color="var(--color-green)" stop-opacity="0" />
        </linearGradient>
      </defs>
      {#each [0.5, 1] as f (f)}
        <line x1="0" x2={W} y1={H - f * (H - 4) - 2} y2={H - f * (H - 4) - 2} stroke="var(--color-line)" stroke-dasharray="3 5" vector-effect="non-scaling-stroke" />
      {/each}
      <path d={path(0, true)} fill="url(#g-down)" />
      <path d={path(1, false)} fill="none" stroke="var(--color-magenta)" stroke-width="1.5" vector-effect="non-scaling-stroke" stroke-linejoin="round" />
      <path d={path(0, false)} fill="none" stroke="var(--color-green)" stroke-width="2" vector-effect="non-scaling-stroke" stroke-linejoin="round" />
    </svg>
    <span class="pointer-events-none absolute top-0 left-1 rounded bg-base/80 px-1 text-[10px] text-muted tabular">{speed(top)}</span>
    <span class="pointer-events-none absolute left-1 rounded bg-base/80 px-1 text-[10px] text-muted tabular" style="top: calc(50% - 6px)">{speed(top / 2)}</span>
    {#if history.length === 0}
      <span class="absolute inset-0 grid place-items-center text-sm text-muted">waiting for data…</span>
    {/if}
  </div>
</section>
