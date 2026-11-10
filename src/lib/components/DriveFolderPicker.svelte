<script lang="ts">
  import { api, type FolderItem } from "../api";
  import { errText, toast } from "../store.svelte";
  import Icon from "../Icon.svelte";
  import Modal from "./Modal.svelte";

  let {
    accountId,
    onpick,
    onclose,
  }: { accountId: string; onpick: (f: { id: string; display: string; driveId: string | null }) => void; onclose: () => void } = $props();

  let trail = $state<FolderItem[]>([]);
  let items = $state<FolderItem[]>([]);
  let shared = $state<FolderItem[]>([]);
  let loading = $state(true);
  let error = $state("");
  let newName = $state("");
  let creating = $state(false);
  let filter = $state("");

  let current = $derived(trail[trail.length - 1]);
  let driveId = $derived(trail[0]?.driveId ?? null);
  let shown = $derived(filter ? items.filter((i) => i.name.toLowerCase().includes(filter.toLowerCase())) : items);

  async function load(f: FolderItem) {
    loading = true;
    error = "";
    try {
      items = await api.listDriveFolders(accountId, f.id);
    } catch (e) {
      error = errText(e);
      items = [];
    }
    loading = false;
  }

  async function start(root?: FolderItem) {
    try {
      const r = root ?? (await api.driveRoot(accountId));
      trail = [r];
      filter = "";
      await load(r);
    } catch (e) {
      error = errText(e);
      loading = false;
    }
  }

  async function enter(f: FolderItem) {
    trail = [...trail, { ...f, driveId }];
    filter = "";
    await load(f);
  }

  async function jump(i: number) {
    trail = trail.slice(0, i + 1);
    filter = "";
    await load(trail[i]);
  }

  async function create() {
    if (!newName.trim() || !current) return;
    try {
      const f = await api.createDriveFolder(accountId, current.id, newName.trim());
      newName = "";
      creating = false;
      await enter(f);
    } catch (e) {
      toast(errText(e), "error");
    }
  }

  start();
  api.listSharedDrives(accountId).then((s) => (shared = s)).catch(() => {});
</script>

<Modal title="Choose a Google Drive folder" subtitle="Files in this folder will be synced with your local folder." width={600} {onclose}>
  <div class="roots">
    <button class:active={!driveId} onclick={() => start()}><Icon name="drive" size={15} /> My Drive</button>
    {#each shared as s (s.id)}
      <button class:active={driveId === s.id} onclick={() => start(s)}><Icon name="accounts" size={15} /> {s.name}</button>
    {/each}
  </div>

  <nav class="crumbs">
    {#each trail as t, i (t.id)}
      {#if i > 0}<Icon name="chevron" size={14} />{/if}
      <button class:last={i === trail.length - 1} onclick={() => jump(i)}>{t.name}</button>
    {/each}
  </nav>

  <div class="toolbar">
    <div class="search"><Icon name="search" size={15} /><input class="input" placeholder="Filter folders" bind:value={filter} /></div>
    {#if creating}
      <input class="input" style="width:200px" placeholder="New folder name" bind:value={newName} onkeydown={(e) => e.key === "Enter" && create()} />
      <button class="btn sm primary" onclick={create}>Create</button>
      <button class="btn sm ghost" onclick={() => (creating = false)}>Cancel</button>
    {:else}
      <button class="btn sm" onclick={() => (creating = true)}><Icon name="plus" size={14} /> New folder</button>
    {/if}
  </div>

  <div class="list">
    {#if loading}
      <div class="empty"><Icon name="sync" class="spin" size={22} /></div>
    {:else if error}
      <div class="notice danger"><Icon name="error" size={16} />{error}</div>
    {:else if shown.length === 0}
      <div class="empty"><Icon name="folder" size={26} /><span>No subfolders here</span></div>
    {:else}
      {#each shown as f (f.id)}
        <button class="item" ondblclick={() => enter(f)} onclick={() => enter(f)}>
          <Icon name="folder" size={17} />
          <span class="ellipsis">{f.name}</span>
          <Icon name="chevron" size={15} class="chev" />
        </button>
      {/each}
    {/if}
  </div>

  {#snippet footer()}
    <span class="sel ellipsis muted">Selected: <strong>{trail.map((t) => t.name).join(" / ")}</strong></span>
    <button class="btn" onclick={onclose}>Cancel</button>
    <button
      class="btn primary"
      disabled={!current}
      onclick={() => onpick({ id: current!.id, display: trail.map((t) => t.name).join("/"), driveId: trail[0]?.driveId ?? null })}>
      Use this folder
    </button>
  {/snippet}
</Modal>

<style>
  .roots {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  .roots button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 11px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text-2);
    cursor: pointer;
  }
  .roots button.active {
    background: var(--accent-soft);
    color: var(--accent);
    border-color: transparent;
    font-weight: 600;
  }
  .crumbs {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 2px;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .crumbs button {
    border: none;
    background: none;
    padding: 3px 6px;
    border-radius: 5px;
    color: var(--text-2);
    cursor: pointer;
  }
  .crumbs button:hover {
    background: var(--hover);
  }
  .crumbs button.last {
    color: var(--text);
    font-weight: 600;
  }
  .toolbar {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-bottom: 10px;
  }
  .search {
    flex: 1;
    position: relative;
    display: flex;
    align-items: center;
    color: var(--text-3);
  }
  .search :global(.icon) {
    position: absolute;
    left: 10px;
  }
  .search .input {
    padding-left: 32px;
    height: 30px;
  }
  .list {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    height: 300px;
    overflow-y: auto;
    padding: 4px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 36px;
    padding: 0 10px;
    border: none;
    border-radius: 6px;
    background: none;
    text-align: left;
    cursor: pointer;
    color: var(--text);
  }
  .item:hover {
    background: var(--hover);
  }
  .item :global(.icon) {
    color: var(--accent);
  }
  .item :global(.chev) {
    margin-left: auto;
    color: var(--text-3);
  }
  .sel {
    margin-right: auto;
    align-self: center;
    max-width: 300px;
  }
</style>
