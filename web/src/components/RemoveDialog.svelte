<script>
  import Modal from "./Modal.svelte";
  import { api } from "../lib/api.js";
  import { run } from "../lib/store.svelte.js";

  let { gid, name, onclose } = $props();

  function remove() {
    run(api.remove(gid));
    onclose();
  }
</script>

<Modal title="Remove download" {onclose} dismissable>
  <p class="break-words">Remove <strong>{name}</strong>?</p>
  <p class="mt-1 text-sm text-muted">Already downloaded data stays on disk.</p>
  {#snippet footer()}
    <button class="rounded-full bg-raised px-4 py-1.5 text-sm font-semibold hover:bg-line" onclick={onclose}>Cancel</button>
    <button data-autofocus class="rounded-full bg-red px-4 py-1.5 text-sm font-semibold text-sunk hover:bg-orange" onclick={remove}>Remove</button>
  {/snippet}
</Modal>
