// Human-readable numbers, matching dlux-core's `fmt` module (so the TUI and web UI agree).

const UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/** 1536 → "1.50 KiB" */
export function bytes(n) {
  if (n < 1024) return `${n} B`;
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  const digits = value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

export const speed = (n) => `${bytes(n)}/s`;

/** Seconds → "42s", "3m 05s", "2h 07m", "3d 04h"; null → "--". */
export function eta(s) {
  if (s == null) return "--";
  const pad = (v) => String(v).padStart(2, "0");
  const days = Math.floor(s / 86400);
  const hours = Math.floor(s / 3600) % 24;
  const mins = Math.floor(s / 60) % 60;
  const secs = s % 60;
  if (days > 0) return `${days}d ${pad(hours)}h`;
  if (hours > 0) return `${hours}h ${pad(mins)}m`;
  if (mins > 0) return `${mins}m ${pad(secs)}s`;
  return `${secs}s`;
}

export const percent = (r) => `${(Math.min(Math.max(r, 0), 1) * 100).toFixed(1)}%`;

/** "1.2 MiB / 4.0 GiB", or just what is known. */
export function size(d) {
  if (d.total > 0) return `${bytes(d.completed)} / ${bytes(d.total)}`;
  if (d.completed > 0) return bytes(d.completed);
  return "size unknown";
}

/** Downloads slower than this get the amber "slow" colour (as in the TUI). */
export const SLOW_SPEED = 256 * 1024;

/** Progress bar colour token: green fast, amber slow, gray paused, red error, teal done. */
export function progressColor(d) {
  switch (d.status) {
    case "active":
      return d.download_speed >= SLOW_SPEED || d.seeding ? "green" : "yellow";
    case "complete":
      return "teal";
    case "error":
      return "red";
    default:
      return "muted";
  }
}

/** Status text, as in the TUI. */
export function statusText(d) {
  if (d.note) return d.note;
  switch (d.status) {
    case "active":
      if (d.kind === "magnet") return "fetching metadata";
      return d.seeding ? "seeding" : "downloading";
    case "waiting":
      return "queued";
    case "error":
      return d.error ? `error: ${d.error}` : "error";
    default:
      return d.status;
  }
}

/** Round up to a nice axis maximum (1, 1.5, 2, 3, 5, 7.5 × 10ⁿ within its binary unit). */
export function niceCeiling(bytesPerSec) {
  let unit = 1;
  while (bytesPerSec / unit >= 1024 && unit < 2 ** 50) unit *= 1024;
  const v = Math.max(1, Math.ceil(bytesPerSec / unit));
  const steps = [1, 1.5, 2, 3, 5, 7.5];
  for (let mag = 1; ; mag *= 10) {
    for (const s of steps) if (s * mag >= v) return s * mag * unit;
  }
}
