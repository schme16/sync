<script lang="ts">
  import { app } from "../store.svelte";
  import Icon from "../Icon.svelte";
  import PairCard from "../components/PairCard.svelte";
</script>

<div class="page">
  <div class="page-head">
    <div>
      <h1>Sync pairs</h1>
      <p>Each pair keeps one folder on this computer in sync with one folder in Google Drive.</p>
    </div>
    <span class="spacer"></span>
    <button class="btn primary" disabled={!app.config?.accounts.length} onclick={() => (app.editingPair = "new")}><Icon name="plus" size={15} /> New sync pair</button>
  </div>

  {#if !app.config?.pairs.length}
    <div class="card">
      <div class="empty">
        <Icon name="folder" size={34} />
        <h3>No sync pairs yet</h3>
        <span>Pick a local folder and a Drive folder, choose two-way or one-way sync, and you're done.</span>
        {#if app.config?.accounts.length}
          <button class="btn primary" onclick={() => (app.editingPair = "new")}><Icon name="plus" size={15} /> New sync pair</button>
        {:else}
          <button class="btn primary" onclick={() => (app.view = "accounts")}>Connect a Google account first</button>
        {/if}
      </div>
    </div>
  {:else}
    <div class="grid">
      {#each app.config.pairs as p (p.id)}<PairCard pair={p} />{/each}
    </div>
  {/if}
</div>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(460px, 1fr));
    gap: 12px;
  }
</style>
