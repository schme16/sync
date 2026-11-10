<script lang="ts">
  import { api, type PairConfig } from "../api";
  import { app, errText, reloadConfig, toast } from "../store.svelte";
  import { ago, count } from "../format";
  import Icon from "../Icon.svelte";
  import StatusPill from "./StatusPill.svelte";

  let { pair }: { pair: PairConfig } = $props();
  let st = $derived(app.snapshot.pairs[pair.id]);
  let transfers = $derived(app.snapshot.transfers.filter((t) => t.pairId === pair.id).length);
  const modeIcon = { twoWay: "twoWay", localToRemote: "up", remoteToLocal: "down" } as const;
  const modeLabel = { twoWay: "Two-way", localToRemote: "Upload only", remoteToLocal: "Download only" } as const;

  async function togglePause() {
    try {
      await api.setPairPaused(pair.id, !pair.paused);
      await reloadConfig();
    } catch (e) {
      toast(errText(e), "error");
    }
  }
</script>

<article class="card pair" class:err={st?.state === "error"}>
  <header>
    <div class="title">
      <h3 class="ellipsis">{pair.name}</h3>
      <StatusPill state={pair.paused || app.snapshot.pausedAll ? "paused" : st?.state} />
    </div>
    <div class="actions">
      <button class="icon-btn" title="Sync now" aria-label="Sync now" onclick={() => api.syncNow(pair.id)}><Icon name="sync" size={16} /></button>
      <button class="icon-btn" title={pair.paused ? "Resume" : "Pause"} aria-label={pair.paused ? "Resume" : "Pause"} onclick={togglePause}>
        <Icon name={pair.paused ? "play" : "pause"} size={16} />
      </button>
      <button class="icon-btn" title="Edit" aria-label="Edit" onclick={() => (app.editingPair = pair.id)}><Icon name="edit" size={16} /></button>
    </div>
  </header>

  <div class="route">
    <button class="end" title="Open folder" onclick={() => api.openLocal(pair.id)}>
      <Icon name="laptop" size={15} />
      <span class="ellipsis mono">{pair.localPath}</span>
    </button>
    <span class="dir" title={modeLabel[pair.mode]}><Icon name={modeIcon[pair.mode]} size={15} /></span>
    <button class="end" title="Open in Google Drive" onclick={() => api.openInDrive(pair.id)}>
      <Icon name="cloud" size={15} />
      <span class="ellipsis mono">{pair.remotePathDisplay}</span>
    </button>
  </div>

  {#if st?.error}
    <div class="notice danger"><Icon name="error" size={16} /><span class="selectable">{st.error}</span></div>
  {/if}

  <footer>
    <span>{modeLabel[pair.mode]}</span>
    <span>·</span>
    <span>{count(st?.localItems ?? 0)} items</span>
    {#if pair.mounts.length}<span>·</span><span><Icon name="link" size={12} /> {pair.mounts.length} external</span>{/if}
    <span class="spacer"></span>
    {#if transfers}<span class="badge accent">{transfers} transferring</span>{/if}
    {#if (st?.queued ?? 0) > 0}<span>{count(st!.queued)} queued</span>{/if}
    {#if (st?.warnings.length ?? 0) > 0}<span class="badge warn" title={st!.warnings.join("\n")}>{st!.warnings.length} warning{st!.warnings.length > 1 ? "s" : ""}</span>{/if}
    <span title={st?.lastSyncMs ? new Date(st.lastSyncMs).toLocaleString() : ""}>Synced {ago(st?.lastSyncMs)}</span>
  </footer>
</article>

<style>
  .pair {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 14px 16px;
  }
  .pair.err {
    border-color: color-mix(in srgb, var(--danger) 45%, var(--border));
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .title {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
    flex: 1;
  }
  h3 {
    font-size: 15px;
  }
  .actions {
    display: flex;
    gap: 2px;
  }
  .route {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
    align-items: center;
    gap: 8px;
  }
  .end {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: 34px;
    padding: 0 10px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-2);
    cursor: pointer;
    text-align: left;
  }
  .end:hover {
    color: var(--text);
    border-color: var(--border-strong);
  }
  .end span {
    font-size: 12px;
  }
  .dir {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
  }
  footer {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 7px;
    color: var(--text-3);
    font-size: 12px;
  }
  footer :global(.icon) {
    display: inline;
    vertical-align: -2px;
  }
</style>
