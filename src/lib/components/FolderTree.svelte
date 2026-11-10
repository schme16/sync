<script lang="ts">
  import Self from "./FolderTree.svelte";
  import { api, type BrowseEntry } from "../api";
  import { errText, toast } from "../store.svelte";
  import Icon from "../Icon.svelte";

  let {
    pairId,
    path = "",
    folderId = null,
    depth = 0,
    onchanged,
  }: { pairId: string; path?: string; folderId?: string | null; depth?: number; onchanged: () => void } = $props();

  let entries = $state<BrowseEntry[] | null>(null);
  let open = $state<Record<string, boolean>>({});
  let busy = $state<string | null>(null);

  async function load() {
    try {
      entries = await api.browsePair(pairId, path, folderId);
    } catch (e) {
      toast(errText(e), "error");
      entries = [];
    }
  }
  load();

  const managed = (e: BrowseEntry) => e.rule === `/${e.path}/`;

  async function toggle(e: BrowseEntry) {
    busy = e.path;
    try {
      await api.setFolderExcluded(pairId, e.path, !e.ignored);
      await load();
      onchanged();
    } catch (err) {
      toast(errText(err), "error");
    }
    busy = null;
  }
</script>

{#if entries === null}
  <div class="loading" style="padding-left:{depth * 22 + 34}px"><Icon name="sync" size={14} class="spin" /> Loading…</div>
{:else if entries.length === 0 && depth === 0}
  <div class="empty"><Icon name="folder" size={26} /><span>No folders yet</span></div>
{:else}
  {#each entries as e (e.path)}
    {@const locked = e.ignored && !managed(e)}
    <div class="node" class:ignored={e.ignored} style="padding-left:{depth * 22 + 8}px">
      <button class="icon-btn exp" aria-label="Expand" disabled={e.ignored} onclick={() => (open[e.path] = !open[e.path])}>
        <Icon name={open[e.path] ? "chevronDown" : "chevron"} size={14} />
      </button>
      <input
        type="checkbox"
        checked={!e.ignored}
        disabled={locked || busy !== null}
        title={locked ? `Excluded by rule ${e.rule}` : e.ignored ? "Include this folder" : "Exclude this folder"}
        onchange={() => toggle(e)} />
      <Icon name="folder" size={16} class="fi" />
      <span class="name ellipsis">{e.name}</span>
      {#if busy === e.path}<Icon name="sync" size={13} class="spin" />{/if}
      {#if !e.local && e.remoteId}<span class="badge" title="Only in Drive so far"><Icon name="cloud" size={12} /> Drive</span>{/if}
      {#if e.local && !e.remoteId}<span class="badge" title="Only on this computer so far"><Icon name="laptop" size={12} /> Local</span>{/if}
      {#if locked}<span class="badge warn mono" title="Change this in the Exclusions tab">{e.rule}</span>{/if}
    </div>
    {#if open[e.path] && !e.ignored}
      <Self {pairId} path={e.path} folderId={e.remoteId} depth={depth + 1} {onchanged} />
    {/if}
  {/each}
{/if}

<style>
  .node {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding-right: 8px;
    border-radius: 6px;
  }
  .node:hover {
    background: var(--hover);
  }
  .exp {
    width: 22px;
    height: 22px;
  }
  .node :global(.fi) {
    color: var(--accent);
  }
  .node.ignored .name,
  .node.ignored :global(.fi) {
    color: var(--text-3);
    text-decoration: line-through;
    text-decoration-color: var(--text-3);
  }
  .name {
    flex: 1;
  }
  input[type="checkbox"] {
    accent-color: var(--accent);
    width: 15px;
    height: 15px;
    margin: 0;
  }
  .loading {
    display: flex;
    gap: 6px;
    align-items: center;
    height: 30px;
    color: var(--text-3);
  }
</style>
