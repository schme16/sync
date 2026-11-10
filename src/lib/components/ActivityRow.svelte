<script lang="ts">
  import type { ActivityItem } from "../api";
  import { app } from "../store.svelte";
  import { basename, dirname, time } from "../format";
  import Icon from "../Icon.svelte";

  let { item, showPair = true }: { item: ActivityItem; showPair?: boolean } = $props();

  const look: Record<ActivityItem["kind"], { icon: string; cls: string }> = {
    upload: { icon: "up", cls: "accent" },
    download: { icon: "down", cls: "ok" },
    deleteLocal: { icon: "trash", cls: "muted" },
    deleteRemote: { icon: "trash", cls: "muted" },
    moveLocal: { icon: "move", cls: "accent" },
    moveRemote: { icon: "move", cls: "accent" },
    mkdir: { icon: "folder", cls: "muted" },
    conflict: { icon: "conflict", cls: "warn" },
    error: { icon: "error", cls: "danger" },
    warning: { icon: "conflict", cls: "warn" },
    info: { icon: "info", cls: "muted" },
  };
  let l = $derived(look[item.kind]);
  let pairName = $derived(app.config?.pairs.find((p) => p.id === item.pairId)?.name ?? "");
</script>

<div class="act {l.cls}">
  <span class="ic"><Icon name={l.icon} size={14} /></span>
  <div class="main">
    {#if item.path}
      <div class="row1">
        <span class="name ellipsis">{basename(item.path)}</span>
        {#if dirname(item.path)}<span class="dir ellipsis mono">{dirname(item.path)}</span>{/if}
      </div>
      <div class="detail ellipsis selectable" title={item.detail}>{item.detail}</div>
    {:else}
      <div class="row1"><span class="name wrap selectable">{item.detail}</span></div>
    {/if}
  </div>
  {#if showPair && pairName}<span class="pair badge">{pairName}</span>{/if}
  <span class="ts" title={new Date(item.ts).toLocaleString()}>{time(item.ts)}</span>
</div>

<style>
  .act {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 16px;
    min-height: 48px;
  }
  .act + :global(.act) {
    border-top: 1px solid var(--border);
  }
  .ic {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    flex: none;
    background: var(--surface-2);
    color: var(--text-2);
  }
  .accent .ic {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .ok .ic {
    background: var(--ok-soft);
    color: var(--ok);
  }
  .warn .ic {
    background: var(--warn-soft);
    color: var(--warn);
  }
  .danger .ic {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .main {
    flex: 1;
    min-width: 0;
  }
  .row1 {
    display: flex;
    gap: 8px;
    align-items: baseline;
    min-width: 0;
  }
  .name {
    font-weight: 500;
    flex: none;
    max-width: 60%;
  }
  .wrap {
    white-space: normal;
    max-width: none;
    flex: 1;
  }
  .dir {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .detail {
    color: var(--text-2);
    font-size: 12.5px;
  }
  .danger .detail {
    color: var(--danger);
  }
  .pair {
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ts {
    color: var(--text-3);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    width: 74px;
    text-align: right;
  }
</style>
