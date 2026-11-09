// The UI's shared state. Components read and change `ui` directly; `snap` is replaced by every
// WebSocket message.

export const TABS = [
  { id: "active", label: "Active" },
  { id: "waiting", label: "Waiting" },
  { id: "done", label: "Done" },
  { id: "errors", label: "Errors" },
];

export const ui = $state({
  /** Latest state from the server, or null before the first message. */
  snap: null,
  /** "connecting" | "open" | "closed" | "unauthorized" */
  link: "connecting",
  tab: "active",
  selected: null,
  /** Details pane open (narrow screens; always shown on wide ones). */
  details: false,
  /** { gid, x, y } */
  menu: null,
  /** { kind: "add" | "limits" | "remove" | "help", ...props } */
  dialog: null,
  toasts: [],
  /** Global limits, once loaded: { download, upload, download_text, upload_text } */
  limits: null,
});

let toastId = 0;

export function toast(text, error = false) {
  const id = ++toastId;
  ui.toasts.push({ id, text, error });
  setTimeout(() => {
    const i = ui.toasts.findIndex((t) => t.id === id);
    if (i >= 0) ui.toasts.splice(i, 1);
  }, error ? 6000 : 4000);
}

/** Run an API call; toast its error (and an optional success message). */
export async function run(promise, success) {
  try {
    const result = await promise;
    if (success) toast(typeof success === "function" ? success(result) : success);
    return result;
  } catch (e) {
    toast(e.message, true);
    return undefined;
  }
}

export const downloads = () => ui.snap?.downloads ?? [];
export const visible = () => downloads().filter((d) => d.tab === ui.tab);
export const find = (gid) => downloads().find((d) => d.gid === gid);
export const selectedDownload = () => (ui.selected ? find(ui.selected) : undefined);

/** Whether a command applies (same rules as the TUI). */
export function can(cmd, d = selectedDownload()) {
  const all = downloads();
  switch (cmd) {
    case "pause":
      return !!d && (d.status === "active" || d.status === "waiting");
    case "resume":
      return !!d && (d.status === "paused" || (d.status === "error" && d.kind === "video"));
    case "remove":
      return !!d;
    case "limit":
      return !!d && !["complete", "error", "removed"].includes(d.status) && d.kind !== "video" && d.kind !== "tool";
    case "copy":
      return !!d?.url;
    case "pauseAll":
      return all.some((x) => x.status === "active" || x.status === "waiting");
    case "resumeAll":
      return all.some((x) => x.status === "paused");
    case "purge":
      return all.some((x) => ["complete", "error", "removed"].includes(x.status));
    default:
      return true;
  }
}
