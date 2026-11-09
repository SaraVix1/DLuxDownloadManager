<script>
  import { onMount, tick } from "svelte";
  import Header from "./components/Header.svelte";
  import SpeedGraph from "./components/SpeedGraph.svelte";
  import Tabs from "./components/Tabs.svelte";
  import DownloadCard from "./components/DownloadCard.svelte";
  import Details from "./components/Details.svelte";
  import Toolbar from "./components/Toolbar.svelte";
  import ContextMenu from "./components/ContextMenu.svelte";
  import Toasts from "./components/Toasts.svelte";
  import AddDialog from "./components/AddDialog.svelte";
  import LimitsDialog from "./components/LimitsDialog.svelte";
  import RemoveDialog from "./components/RemoveDialog.svelte";
  import HelpDialog from "./components/HelpDialog.svelte";
  import SignIn from "./components/SignIn.svelte";
  import { api, live } from "./lib/api.js";
  import { command, openAdd } from "./lib/commands.js";
  import { speed } from "./lib/format.js";
  import { TABS, toast, ui, visible } from "./lib/store.svelte.js";

  const EMPTY = {
    active: "No active downloads",
    waiting: "Nothing waiting",
    done: "Nothing finished yet",
    errors: "No errors",
  };

  let wide = $state(matchMedia("(min-width: 1024px)").matches);
  let dragging = $state(false);
  let list = $state();
  let lastIndex = 0;

  const items = $derived(visible());

  // Keep a selection, like the TUI: the same download if it is still listed, otherwise
  // whatever now sits where it was.
  $effect(() => {
    const i = items.findIndex((d) => d.gid === ui.selected);
    if (i >= 0) lastIndex = i;
    else ui.selected = items[Math.min(lastIndex, items.length - 1)]?.gid ?? null;
  });

  // Live speed in the tab title.
  $effect(() => {
    const s = ui.snap?.speed.download ?? 0;
    document.title = s > 0 ? `↓ ${speed(s)} · dlux` : "dlux";
  });

  async function loadLimits() {
    try {
      ui.limits = await api.limits();
    } catch {
      // Shown when the user opens the dialog.
    }
  }

  onMount(() => {
    // The token was only needed to set the cookie; don't leave it in the address bar.
    if (new URLSearchParams(location.search).has("token")) history.replaceState(null, "", location.pathname);
    const media = matchMedia("(min-width: 1024px)");
    const onMedia = () => (wide = media.matches);
    media.addEventListener("change", onMedia);
    const stop = live({
      onState: (s) => (ui.snap = s),
      onStatus: (s) => {
        ui.link = s;
        if (s === "open") loadLimits();
      },
    });
    return () => {
      stop();
      media.removeEventListener("change", onMedia);
    };
  });

  function editing(t) {
    return t instanceof HTMLElement && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName));
  }

  async function move(to) {
    const i = items.findIndex((d) => d.gid === ui.selected);
    const next = Math.max(0, Math.min(items.length - 1, to(i < 0 ? 0 : i)));
    ui.selected = items[next]?.gid ?? null;
    await tick();
    list?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }

  const KEYS = {
    j: () => move((i) => i + 1),
    ArrowDown: () => move((i) => i + 1),
    k: () => move((i) => i - 1),
    ArrowUp: () => move((i) => i - 1),
    g: () => move(() => 0),
    Home: () => move(() => 0),
    G: () => move(() => Infinity),
    End: () => move(() => Infinity),
    Enter: () => command("details"),
    a: () => command("add"),
    p: () => command("pause"),
    r: () => command("resume"),
    " ": () => command("toggle"),
    d: () => command("remove"),
    Delete: () => command("remove"),
    P: () => command("pauseAll"),
    R: () => command("resumeAll"),
    X: () => command("purge"),
    s: () => command("limit"),
    S: () => command("globalLimit"),
    c: () => command("copy"),
    U: () => command("update"),
    "?": () => command("help"),
    Escape: () => (ui.details = false),
  };

  function keydown(e) {
    if (ui.dialog || ui.menu || e.ctrlKey || e.metaKey || e.altKey || editing(e.target)) return;
    const tab = "1234".indexOf(e.key);
    if (tab >= 0) {
      ui.tab = TABS[tab].id;
      e.preventDefault();
      return;
    }
    const action = KEYS[e.key];
    if (action) {
      e.preventDefault();
      action();
    }
  }

  function paste(e) {
    if (ui.dialog || editing(e.target)) return;
    const text = e.clipboardData?.getData("text");
    if (text?.trim()) {
      e.preventDefault();
      openAdd(text);
    }
  }

  // Drag-and-drop: .torrent/.metalink files are uploaded, links open the Add dialog.
  const acceptsDrop = (e) => [...(e.dataTransfer?.types ?? [])].some((t) => t === "Files" || t === "text/uri-list" || t === "text/plain");

  function dragover(e) {
    if (!acceptsDrop(e) || ui.dialog) return;
    e.preventDefault();
    dragging = true;
  }

  async function drop(e) {
    if (!acceptsDrop(e) || ui.dialog) return;
    e.preventDefault();
    dragging = false;
    const files = [...e.dataTransfer.files];
    if (files.length) {
      let added = 0;
      for (const f of files) {
        if (!/\.(torrent|metalink|meta4)$/i.test(f.name)) {
          toast(`${f.name}: only .torrent and .metalink files can be dropped`, true);
          continue;
        }
        try {
          await api.upload(f, ui.snap?.default_dir);
          added++;
        } catch (err) {
          toast(`${f.name}: ${err.message}`, true);
        }
      }
      if (added) {
        toast(added === 1 ? `Added ${files[0].name}` : `Added ${added} files`);
        ui.tab = "active";
      }
      return;
    }
    const text = e.dataTransfer.getData("text/uri-list") || e.dataTransfer.getData("text/plain");
    if (text.trim()) openAdd(text.split(/\r?\n/).filter((l) => !l.startsWith("#")).join("\n"));
  }

  const closeDialog = () => {
    const wasLimits = ui.dialog?.kind === "limits";
    ui.dialog = null;
    if (wasLimits) loadLimits();
  };
</script>

<svelte:window
  onkeydown={keydown}
  onpaste={paste}
  ondragover={dragover}
  ondragleave={(e) => e.relatedTarget === null && (dragging = false)}
  ondrop={drop}
/>

{#if ui.link === "unauthorized"}
  <SignIn />
{:else}
  <div class="mx-auto flex h-dvh max-w-[1600px] flex-col gap-3 p-3 sm:p-4">
    <Header />
    <div class="hidden [@media(min-height:560px)]:block"><SpeedGraph /></div>
    <Tabs />

    <div class="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_minmax(22rem,30rem)]">
      <section class="flex min-h-0 flex-col rounded-2xl border border-line" aria-label="Downloads">
        <div class="flex items-center px-4 pt-3 pb-1 text-sm">
          <h2 class="font-bold text-blue">Downloads</h2>
          {#if items.length}
            <span class="ml-auto text-muted tabular">{Math.max(1, items.findIndex((d) => d.gid === ui.selected) + 1)}/{items.length}</span>
          {/if}
        </div>
        {#if ui.link !== "open" && !ui.snap}
          <p class="grid flex-1 place-items-center text-sm text-muted">Connecting to dlux…</p>
        {:else if items.length === 0}
          <div class="grid flex-1 place-items-center px-4 text-center">
            <div>
              <p class="text-fg-dim">{EMPTY[ui.tab]}</p>
              {#if ui.tab === "active"}
                <p class="mt-1 text-sm text-muted">Press <kbd class="text-blue">a</kbd>, paste a link, or drop a .torrent here</p>
              {/if}
            </div>
          </div>
        {:else}
          <ul bind:this={list} class="min-h-0 flex-1 space-y-1 overflow-y-auto px-2 pb-2" role="listbox" aria-label="{ui.tab} downloads">
            {#each items as d (d.gid)}
              <DownloadCard {d} onopen={() => (ui.details = true)} />
            {/each}
          </ul>
        {/if}
        {#if ui.link === "closed"}
          <p class="mx-3 mb-3 rounded-lg bg-red/10 px-3 py-1.5 text-sm text-red">Connection to dlux lost · reconnecting…</p>
        {:else if ui.snap && !ui.snap.aria2.connected}
          <p class="mx-3 mb-3 rounded-lg bg-red/10 px-3 py-1.5 text-sm text-red">aria2 is not responding{ui.snap.aria2.error ? `: ${ui.snap.aria2.error}` : ""}</p>
        {/if}
      </section>
      {#if wide}<Details />{/if}
    </div>

    <Toolbar />
  </div>

  {#if !wide && ui.details}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="fixed inset-0 z-30 flex flex-col justify-end bg-black/50" onclick={(e) => e.target === e.currentTarget && (ui.details = false)}>
      <div class="max-h-[75dvh] rounded-t-2xl bg-sunk p-2"><Details onclose={() => (ui.details = false)} /></div>
    </div>
  {/if}

  {#if dragging}
    <div class="pointer-events-none fixed inset-3 z-[70] grid place-items-center rounded-3xl border-2 border-dashed border-blue bg-base/85">
      <p class="text-lg font-semibold text-blue">Drop .torrent / .metalink files or links to add them</p>
    </div>
  {/if}

  <ContextMenu />
  {#if ui.dialog?.kind === "add"}<AddDialog text={ui.dialog.text} onclose={closeDialog} />{/if}
  {#if ui.dialog?.kind === "limits"}<LimitsDialog gid={ui.dialog.gid} name={ui.dialog.name} onclose={closeDialog} />{/if}
  {#if ui.dialog?.kind === "remove"}<RemoveDialog gid={ui.dialog.gid} name={ui.dialog.name} onclose={closeDialog} />{/if}
  {#if ui.dialog?.kind === "help"}<HelpDialog onclose={closeDialog} />{/if}
  <Toasts />
{/if}
