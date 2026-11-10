<script lang="ts">
  import { api, openUrl } from "../api";
  import { app, errText, toast } from "../store.svelte";
  import { bytes } from "../format";
  import Icon from "../Icon.svelte";

  let u = $derived(app.update);
  let info = $derived(u?.available ?? null);
  let working = $derived(!!u && (!!u.progress || u.installing));
  let visible = $derived(!!info && (working || app.updateDismissed !== info.version));
  let notesOpen = $state(false);

  async function install() {
    try {
      await api.installUpdate();
    } catch (e) {
      toast(`Update failed: ${errText(e)}`, "error");
    }
  }

  async function skip() {
    if (!info) return;
    const v = info.version;
    try {
      await api.skipUpdate(v);
      toast(`Sync won't remind you about ${v}. You can still update from Settings.`, "info");
    } catch (e) {
      toast(errText(e), "error");
    }
  }
</script>

<div class="slot">
  {#if visible && info && u}
    <div class="banner" role="status">
      <span class="ico"><Icon name="down" size={18} /></span>
      <div class="text">
        {#if u.installing}
          <strong>Installing Sync {info.version}…</strong>
          <span class="muted">Sync will close and reopen in a moment.</span>
        {:else if u.progress}
          <strong>Downloading Sync {info.version}…</strong>
          <span class="muted">{bytes(u.progress[0])} of {bytes(u.progress[1])}</span>
          <div class="bar"><div style="width:{u.progress[1] ? Math.round((u.progress[0] / u.progress[1]) * 100) : 0}%"></div></div>
        {:else}
          <strong>Sync {info.version} is available</strong>
          <span class="muted">
            You have {u.current}.
            {#if info.notes}<button class="lnk" onclick={() => (notesOpen = !notesOpen)}>{notesOpen ? "Hide" : "What's new"}</button>{/if}
            {#if info.manualReason}<br />{info.manualReason}{/if}
          </span>
        {/if}
      </div>
      {#if !working}
        <div class="actions">
          <button class="btn sm ghost" onclick={() => (app.updateDismissed = info.version)}>Later</button>
          <button class="btn sm ghost" onclick={skip}>Skip this version</button>
          {#if info.manualReason}
            <button class="btn sm primary" onclick={() => openUrl(info.url)}><Icon name="external" size={14} /> Open release page</button>
          {:else}
            <button class="btn sm primary" onclick={install}>Install and restart</button>
          {/if}
        </div>
      {/if}
    </div>
    {#if notesOpen && info.notes && !working}
      <pre class="notes selectable">{info.notes}</pre>
    {/if}
  {/if}
</div>

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 32px;
    background: var(--accent-soft);
    border-bottom: 1px solid color-mix(in srgb, var(--accent) 25%, var(--border));
  }
  .ico {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    background: var(--accent);
    color: #fff;
    flex: none;
  }
  .text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .text .muted {
    font-size: 12.5px;
  }
  .actions {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .lnk {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
    font-weight: 600;
  }
  .lnk:hover {
    text-decoration: underline;
  }
  .bar {
    height: 4px;
    max-width: 360px;
    margin-top: 4px;
    border-radius: 4px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--accent);
    transition: width 0.2s ease;
  }
  .notes {
    margin: 0;
    padding: 12px 32px 14px 74px;
    max-height: 220px;
    overflow: auto;
    white-space: pre-wrap;
    font: inherit;
    font-size: 12.5px;
    color: var(--text-2);
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
</style>
