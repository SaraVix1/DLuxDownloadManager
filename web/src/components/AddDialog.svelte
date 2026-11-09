<script>
  import Modal from "./Modal.svelte";
  import { api } from "../lib/api.js";
  import { toast, ui } from "../lib/store.svelte.js";

  let { text: initial = "", onclose } = $props();

  // The initial text only pre-fills the field.
  // svelte-ignore state_referenced_locally
  let text = $state(initial);
  let dir = $state(ui.snap?.default_dir ?? "");
  let paused = $state(false);
  /** null = decide automatically from the input */
  let videoChoice = $state(null);
  let check = $state(null);
  let error = $state("");
  let busy = $state(false);

  // Video step.
  let step = $state("form");
  let url = $state("");
  let probe = $state({ loading: false, info: null, error: "" });
  let cursor = $state(0);

  // Live preview from the server's parser (paths are checked where aria2 runs).
  let timer;
  $effect(() => {
    const value = text;
    clearTimeout(timer);
    if (!value.trim()) {
      check = null;
      return;
    }
    timer = setTimeout(async () => {
      try {
        check = await api.check(value);
      } catch {
        check = null;
      }
    }, 200);
  });

  const video = $derived(videoChoice ?? !!check?.video);
  const sources = $derived(check?.sources ?? []);
  const unsupported = $derived(probe.error.includes("doesn't recognise"));

  async function submit() {
    error = "";
    if (video) {
      if (sources.length !== 1 || sources[0].kind !== "URL" || check?.invalid.length) {
        error = "Video mode takes a single http(s) URL";
        return;
      }
      url = sources[0].text;
      step = "video";
      return fetchFormats();
    }
    busy = true;
    try {
      const r = await api.add(text, dir, paused);
      toast(r.message, r.failed.length > 0);
      ui.tab = paused ? "waiting" : "active";
      onclose();
    } catch (e) {
      error = e.message;
    } finally {
      busy = false;
    }
  }

  async function fetchFormats() {
    probe = { loading: true, info: null, error: "" };
    cursor = 0;
    try {
      probe = { loading: false, info: await api.probe(url), error: "" };
    } catch (e) {
      probe = { loading: false, info: null, error: e.message };
    }
  }

  async function download() {
    const c = probe.info?.choices[cursor];
    if (!c) return;
    busy = true;
    try {
      await api.startVideo({
        url,
        selector: c.selector,
        title: probe.info.title,
        dir,
        playlist: probe.info.playlist,
        size: c.size,
        needs_merge: c.needs_merge,
      });
      ui.tab = "active";
      onclose();
    } catch (e) {
      probe.error = e.message;
    } finally {
      busy = false;
    }
  }

  async function asFile() {
    busy = true;
    try {
      const r = await api.add(url, dir, false);
      toast(r.message, r.failed.length > 0);
      ui.tab = "active";
      onclose();
    } catch (e) {
      probe.error = e.message;
    } finally {
      busy = false;
    }
  }

  async function updateYtDlp() {
    toast("Updating yt-dlp…");
    try {
      toast((await api.updateYtDlp()).message);
    } catch (e) {
      toast(e.message, true);
    }
  }

  async function upload(e) {
    const files = [...e.currentTarget.files];
    e.currentTarget.value = "";
    for (const f of files) {
      try {
        await api.upload(f, dir, paused);
        toast(`Added ${f.name}`);
      } catch (err) {
        toast(`${f.name}: ${err.message}`, true);
      }
    }
    if (files.length) {
      ui.tab = paused ? "waiting" : "active";
      onclose();
    }
  }

  function keydown(e) {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    }
  }

  function choiceKey(e) {
    const n = probe.info?.choices.length ?? 0;
    if (e.key === "ArrowDown" || e.key === "j") cursor = Math.min(cursor + 1, n - 1);
    else if (e.key === "ArrowUp" || e.key === "k") cursor = Math.max(cursor - 1, 0);
    else if (e.key === "Enter") download();
    else return;
    e.preventDefault();
  }
</script>

{#if step === "form"}
  <Modal title="Add downloads" {onclose} wide>
    <label class="mb-1 block text-sm font-semibold text-fg-dim" for="add-text">URLs, magnet links or .torrent / .metalink files</label>
    <textarea
      id="add-text"
      data-autofocus
      bind:value={text}
      onkeydown={keydown}
      rows="5"
      spellcheck="false"
      placeholder="One per line · paste, or drop files anywhere on the page"
      class="w-full resize-y rounded-xl border border-line bg-base px-3 py-2 font-mono text-sm placeholder:text-muted focus:border-line-focus focus:outline-none"
    ></textarea>
    <p class="mt-1 min-h-5 text-sm" aria-live="polite">
      {#if error}
        <span class="text-red">✗ {error}</span>
      {:else if !text.trim()}
        <span class="text-muted">Each line is one download · Shift+Enter for a new line</span>
      {:else if check?.invalid.length}
        <span class="text-yellow">! {check.invalid[0].input}: {check.invalid[0].reason}</span>
      {:else if video && sources.length === 1}
        <span class="text-magenta">▶ Video page · Next fetches the available formats</span>
      {:else if check}
        <span class="text-green">✓ {check.summary}</span>
      {/if}
    </p>

    <label class="mt-3 mb-1 block text-sm font-semibold text-fg-dim" for="add-dir">Save to <span class="font-normal text-muted">(on the server)</span></label>
    <input
      id="add-dir"
      bind:value={dir}
      spellcheck="false"
      placeholder="aria2's default directory"
      class="w-full rounded-xl border border-line bg-base px-3 py-2 font-mono text-sm placeholder:text-muted focus:border-line-focus focus:outline-none"
    />

    <div class="mt-3 flex flex-wrap gap-x-6 gap-y-2 text-sm">
      <label class="flex items-center gap-2 {video ? 'text-muted' : 'text-fg-dim'}">
        <input type="checkbox" bind:checked={paused} disabled={video} class="accent-blue" /> Start paused
      </label>
      <label class="flex items-center gap-2 text-fg-dim">
        <input type="checkbox" checked={video} onchange={(e) => (videoChoice = e.currentTarget.checked)} class="accent-magenta" />
        Video: choose quality with yt-dlp
      </label>
    </div>
    {#snippet footer()}
      <label class="mr-auto cursor-pointer rounded-full px-3 py-1.5 text-sm text-fg-dim hover:bg-raised hover:text-fg">
        Upload .torrent…
        <input type="file" accept=".torrent,.metalink,.meta4" multiple class="sr-only" onchange={upload} />
      </label>
      <button class="rounded-full bg-raised px-4 py-1.5 text-sm font-semibold hover:bg-line" onclick={onclose}>Cancel</button>
      <button class="rounded-full bg-blue px-4 py-1.5 text-sm font-semibold text-sunk hover:bg-cyan disabled:opacity-50" disabled={busy || !text.trim()} onclick={submit}>
        {video ? "Next ›" : "Add"}
      </button>
    {/snippet}
  </Modal>
{:else}
  <Modal title="Download video" {onclose} wide>
    <p class="font-semibold break-words">{probe.info?.title ?? url}</p>
    {#if probe.loading}
      <p class="mt-1 flex items-center gap-2 text-sm text-muted">
        <span class="size-3 animate-spin rounded-full border-2 border-blue border-t-transparent"></span>
        Fetching formats with yt-dlp…
      </p>
    {:else if probe.error}
      <p class="mt-1 text-sm text-red">✗ {probe.error}</p>
      {#if unsupported}<p class="mt-1 text-sm text-muted">It may still be a plain file: try “Download as file”.</p>{/if}
    {:else if probe.info}
      <p class="mt-0.5 text-sm text-muted">{probe.info.subtitle}</p>
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <ul class="mt-3 space-y-1" role="listbox" aria-label="Formats" tabindex="0" data-autofocus onkeydown={choiceKey}>
        {#each probe.info.choices as c, i (c.selector + c.label)}
          <li
            role="option"
            aria-selected={i === cursor}
            class="flex cursor-pointer items-center gap-3 rounded-lg px-3 py-2 text-sm {i === cursor ? 'bg-raised text-blue' : 'hover:bg-hover'}"
            onclick={() => (cursor = i)}
            ondblclick={download}
            onkeydown={() => {}}
          >
            <span class="size-2 rounded-full {i === cursor ? 'bg-blue' : 'bg-line'}"></span>
            <span class="font-semibold">{c.label}</span>
            <span class="ml-auto text-muted tabular">{c.detail}</span>
          </li>
        {/each}
      </ul>
    {/if}
    <p class="mt-3 text-sm text-muted">Save to <span class="font-mono text-fg-dim">{dir || "aria2's default directory"}</span></p>
    {#snippet footer()}
      <button class="mr-auto rounded-full px-3 py-1.5 text-sm text-fg-dim hover:bg-raised hover:text-fg" onclick={() => (step = "form")}>‹ Back</button>
      <button class="rounded-full bg-raised px-4 py-1.5 text-sm font-semibold hover:bg-line" onclick={updateYtDlp}>Update yt-dlp</button>
      <button class="rounded-full bg-raised px-4 py-1.5 text-sm font-semibold hover:bg-line" onclick={onclose}>Cancel</button>
      {#if unsupported}
        <button class="rounded-full bg-blue px-4 py-1.5 text-sm font-semibold text-sunk hover:bg-cyan" disabled={busy} onclick={asFile}>Download as file</button>
      {:else if probe.error}
        <button class="rounded-full bg-blue px-4 py-1.5 text-sm font-semibold text-sunk hover:bg-cyan" onclick={fetchFormats}>Retry</button>
      {:else}
        <button class="rounded-full bg-blue px-4 py-1.5 text-sm font-semibold text-sunk hover:bg-cyan disabled:opacity-50" disabled={busy || !probe.info} onclick={download}>Download</button>
      {/if}
    {/snippet}
  </Modal>
{/if}
