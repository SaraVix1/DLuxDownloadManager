// Every user command, shared by keyboard shortcuts, toolbar buttons and the context menu.

import { api } from "./api.js";
import { can, run, selectedDownload, toast, ui } from "./store.svelte.js";

const STOPPED = ["complete", "error", "removed"];

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // navigator.clipboard needs a secure context (plain http on a LAN address isn't one).
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.append(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    if (!ok) throw new Error("The browser refused to copy");
  }
}

export function openAdd(text = "") {
  ui.menu = null;
  ui.dialog = { kind: "add", text };
}

/** Run `cmd` for download `d` (default: the selection). Disabled commands do nothing. */
export async function command(cmd, d = selectedDownload()) {
  ui.menu = null;
  if (!can(cmd, d)) return;
  switch (cmd) {
    case "add":
      return openAdd();
    case "pause":
      return run(api.pause(d.gid));
    case "resume":
      return run(api.resume(d.gid));
    case "toggle":
      return command(can("pause", d) ? "pause" : "resume", d);
    case "remove":
      if (STOPPED.includes(d.status)) return run(api.remove(d.gid));
      ui.dialog = { kind: "remove", gid: d.gid, name: d.name };
      return;
    case "pauseAll":
      return run(api.pauseAll(), "Paused all downloads");
    case "resumeAll":
      return run(api.resumeAll(), "Resumed all downloads");
    case "purge":
      return run(api.purge(), "Cleared finished downloads");
    case "limit":
      ui.dialog = { kind: "limits", gid: d.gid, name: d.name };
      return;
    case "globalLimit":
      ui.dialog = { kind: "limits", gid: null, name: "All downloads" };
      return;
    case "copy":
      try {
        await copy(d.url);
        toast("Copied URL to the clipboard");
      } catch (e) {
        toast(e.message, true);
      }
      return;
    case "update":
      toast("Updating yt-dlp…");
      return run(api.updateYtDlp(), (r) => r.message);
    case "details":
      ui.details = !ui.details;
      return;
    case "help":
      ui.dialog = { kind: "help" };
      return;
  }
}

export const MENU = [
  { cmd: "pause", label: "Pause", key: "p" },
  { cmd: "resume", label: "Resume", key: "r" },
  { cmd: "remove", label: "Remove", key: "d" },
  { cmd: "limit", label: "Speed limit…", key: "s" },
  { cmd: "copy", label: "Copy URL", key: "c" },
];

export const TOOLBAR = [
  { cmd: "add", label: "+ Add", primary: true },
  { cmd: "pause", label: "Pause" },
  { cmd: "resume", label: "Resume" },
  { cmd: "remove", label: "Remove" },
  { cmd: "pauseAll", label: "Pause all", wide: true },
  { cmd: "resumeAll", label: "Resume all", wide: true },
  { cmd: "purge", label: "Clear finished", wide: true },
];
