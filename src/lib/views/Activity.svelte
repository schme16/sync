<script lang="ts">
  import { api, type ActivityKind } from "../api";
  import { app } from "../store.svelte";
  import Icon from "../Icon.svelte";
  import ActivityRow from "../components/ActivityRow.svelte";
  import Segmented from "../components/Segmented.svelte";

  type F = "all" | "transfers" | "deletes" | "moves" | "conflict" | "error";
  let filter = $state<F>((app.activityFilter as F) ?? "all");
  let pair = $state("");
  let q = $state("");
  let limit = $state(300);
  app.activityFilter = null;

  const groups: Record<F, ActivityKind[] | null> = {
    all: null,
    transfers: ["upload", "download", "mkdir"],
    deletes: ["deleteLocal", "deleteRemote"],
    moves: ["moveLocal", "moveRemote"],
    conflict: ["conflict", "warning"],
    error: ["error"],
  };

  let list = $derived(
    app.activity.filter(
      (a) =>
        (!groups[filter] || groups[filter]!.includes(a.kind)) &&
        (!pair || a.pairId === pair) &&
        (!q || a.path.toLowerCase().includes(q.toLowerCase()) || a.detail.toLowerCase().includes(q.toLowerCase())),
    ),
  );
</script>

<div class="page">
  <div class="page-head">
    <div>
      <h1>Activity</h1>
      <p>Everything Sync has done recently. Full logs are kept for 7 days.</p>
    </div>
    <span class="spacer"></span>
    <button class="btn" onclick={() => api.openLogDir()}><Icon name="logs" size={15} /> Open logs</button>
    <button class="btn" onclick={async () => { await api.clearActivity(); app.activity = []; }}>Clear</button>
  </div>

  <div class="filters">
    <Segmented
      bind:value={filter}
      options={[
        { value: "all", label: "All" },
        { value: "transfers", label: "Transfers" },
        { value: "moves", label: "Moves" },
        { value: "deletes", label: "Deletions" },
        { value: "conflict", label: "Conflicts" },
        { value: "error", label: "Errors" },
      ]} />
    <span class="spacer"></span>
    <select class="select" style="width:180px" bind:value={pair}>
      <option value="">All sync pairs</option>
      {#each app.config?.pairs ?? [] as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
    </select>
    <div class="search"><Icon name="search" size={15} /><input class="input" placeholder="Search files" bind:value={q} /></div>
  </div>

  <div class="card list">
    {#if list.length === 0}
      <div class="empty"><Icon name="activity" size={28} /><h3>Nothing here yet</h3><span>Activity appears as files are synced.</span></div>
    {:else}
      {#each list.slice(0, limit) as a (a.id)}<ActivityRow item={a} />{/each}
      {#if list.length > limit}
        <div class="more"><button class="btn sm" onclick={() => (limit += 500)}>Show more ({list.length - limit} remaining)</button></div>
      {/if}
    {/if}
  </div>
</div>

<style>
  .filters {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 14px;
    flex-wrap: wrap;
  }
  .search {
    position: relative;
    display: flex;
    align-items: center;
    color: var(--text-3);
    width: 220px;
  }
  .search :global(.icon) {
    position: absolute;
    left: 10px;
  }
  .search .input {
    padding-left: 32px;
  }
  .list {
    padding: 4px 0;
  }
  .more {
    display: flex;
    justify-content: center;
    padding: 12px;
    border-top: 1px solid var(--border);
  }
</style>
