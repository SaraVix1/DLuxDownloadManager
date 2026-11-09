<script>
  let token = $state("");
  let error = $state("");

  async function submit(e) {
    e.preventDefault();
    error = "";
    // A valid ?token= makes the server set an HttpOnly cookie; then reload without it.
    const res = await fetch(`/api/state?token=${encodeURIComponent(token)}`, { credentials: "same-origin" });
    if (res.ok) location.replace("/");
    else error = "That token isn't right";
  }
</script>

<main class="grid min-h-dvh place-items-center p-4">
  <form class="w-full max-w-sm rounded-2xl border border-line bg-sunk p-6" onsubmit={submit}>
    <p class="text-2xl font-bold"><span class="text-purple">◆</span> <span class="text-blue">d</span><span class="text-cyan">l</span><span class="text-teal">u</span><span class="text-magenta">x</span></p>
    <p class="mt-1 mb-5 text-sm text-muted">This dlux needs its web token (<code class="text-fg-dim">[web] token</code> in the config).</p>
    <label for="token" class="mb-1 block text-sm font-semibold text-fg-dim">Token</label>
    <!-- svelte-ignore a11y_autofocus -->
    <input id="token" type="password" autocomplete="current-password" autofocus bind:value={token} class="w-full rounded-xl border border-line bg-base px-3 py-2 focus:border-line-focus focus:outline-none" />
    <p class="mt-2 min-h-5 text-sm text-red">{error}</p>
    <button class="mt-2 w-full rounded-full bg-blue py-2 font-semibold text-sunk hover:bg-cyan disabled:opacity-50" disabled={!token}>Sign in</button>
  </form>
</main>
