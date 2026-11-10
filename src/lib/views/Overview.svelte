<script lang="ts">
  import { api } from "../api";
  import { app } from "../store.svelte";
  import { bytes, count, rate, basename, dirname } from "../format";
  import Icon from "../Icon.svelte";
  import PairCard from "../components/PairCard.svelte";
  import ActivityRow from "../components/ActivityRow.svelte";

  let pairs = $derived(app.config?.pairs ?? []);
  let statuses = $derived(Object.values(app.snapshot.pairs));
  let items = $derived(statuses.reduce((n, s) => n + s.localItems, 0));
  let queued = $derived(statuses.reduce((n, s) => n + s.queued, 0));
  let errors = $derived(statuses.reduce((n, s) => n + s.recentErrors, 0));
  let transfers = $derived(app.snapshot.transfers);
  let busy = $derived(transfers.length > 0 || statuses.some((s) => ["syncing", "scanning", "starting"].includes(s.state)));
  let attention = $derived(statuses.some((s) => s.state === "error"));
  let confirm = $derived(pairs.filter((p) => app.snapshot.pairs[p.id]?.pendingDeletes));
  let speedUp = $derived(transfers.filter((t) => t.upload).reduce((n, t) => n + t.done / Math.max(0.5, (Date.now() - t.startedMs) / 1000), 0));
  let speedDown = $derived(transfers.filter((t) => !t.upload).reduce((n, t) => n + t.done / Math.max(0.5, (Date.now() - t.startedMs) / 1000), 0));

  let headline = $derived(
    app.snapshot.pausedAll
      ? "Syncing is paused"
      : attention
        ? "Some folders need attention"
        : busy
          ? "Syncing your files…"
          : pairs.length
            ? "Everything is up to date"
            : "Let's set up your first sync",
  );
</script>

<div class="page">
  <div class="hero card" class:busy class:paused={app.snapshot.pausedAll} class:attention>
    <div class="orb">
      <Icon name={app.snapshot.pausedAll ? "pause" : attention ? "error" : busy ? "sync" : "check"} size={26} class={busy && !app.snapshot.pausedAll ? "spin" : ""} />
    </div>
    <div class="hero-text">
      <h1>{headline}</h1>
      <p class="muted">
        {#if pairs.length === 0}
          Connect a local folder to Google Drive to get started.
        {:else}
          {pairs.length} sync pair{pairs.length === 1 ? "" : "s"} · {count(items)} items tracked
          {#if transfers.length}· {transfers.length} active transfer{transfers.length === 1 ? "" : "s"}{/if}
        {/if}
      </p>
    </div>
    <div class="hero-actions">
      {#if pairs.length === 0}
        <button class="btn primary" onclick={() => (app.editingPair = "new")} disabled={!app.config?.accounts.length}><Icon name="plus" size={15} /> Add sync pair</button>
      {:else}
        <button class="btn" onclick={() => api.syncNow()}><Icon name="sync" size={15} /> Sync now</button>
        <button class="btn" onclick={() => api.setPausedAll(!app.snapshot.pausedAll)}>
          <Icon name={app.snapshot.pausedAll ? "play" : "pause"} size={15} />
          {app.snapshot.pausedAll ? "Resume" : "Pause"}
        </button>
      {/if}
    </div>
  </div>

  {#each confirm as p (p.id)}
    {@const pd = app.snapshot.pairs[p.id].pendingDeletes!}
    <div class="confirm card">
      <Icon name="shield" size={22} />
      <div class="grow">
        <h3>“{p.name}” wants to delete {count(pd.local + pd.remote)} items</h3>
        <p class="muted">
          {#if pd.remote}{count(pd.remote)} in Google Drive{/if}{#if pd.remote && pd.local} and {/if}{#if pd.local}{count(pd.local)} on this computer{/if}.
          That's more than your safety limit, so nothing has been deleted yet. If this is unexpected (an unplugged drive, an accidental mass delete), choose Restore.
        </p>
        <p class="sample mono faint">{pd.sample.slice(0, 5).join(" · ")}{pd.sample.length > 5 ? " …" : ""}</p>
      </div>
      <div class="col">
        <button class="btn danger-solid" onclick={() => api.confirmDeletes(p.id, true)}>Delete them</button>
        <button class="btn" onclick={() => api.confirmDeletes(p.id, false)}>Restore instead</button>
      </div>
    </div>
  {/each}

  {#if pairs.length}
    <div class="stats">
      <div class="stat card"><span class="k">Items tracked</span><span class="v">{count(items)}</span></div>
      <div class="stat card"><span class="k">Queued</span><span class="v">{count(queued)}</span></div>
      <div class="stat card"><span class="k"><Icon name="up" size={13} /> Upload</span><span class="v">{speedUp > 0 ? `${bytes(speedUp)}/s` : "—"}</span></div>
      <div class="stat card"><span class="k"><Icon name="down" size={13} /> Download</span><span class="v">{speedDown > 0 ? `${bytes(speedDown)}/s` : "—"}</span></div>
      <div class="stat card" class:bad={errors > 0}>
        <span class="k">Recent errors</span>
        <button class="v linkish" onclick={() => { app.activityFilter = "error"; app.view = "activity"; }}>{count(errors)}</button>
      </div>
    </div>
  {/if}

  {#if transfers.length}
    <h2 class="section-title">Transferring now</h2>
    <div class="card transfers">
      {#each transfers.slice(0, 12) as t (t.id)}
        {@const pct = t.total ? Math.round((t.done / t.total) * 100) : 100}
        <div class="tr">
          <span class="dir" class:up={t.upload}><Icon name={t.upload ? "up" : "down"} size={14} /></span>
          <div class="tr-main">
            <div class="row">
              <span class="ellipsis name">{basename(t.path)}</span>
              <span class="faint ellipsis mono small">{dirname(t.path)}</span>
              <span class="spacer"></span>
              <span class="faint small nowrap">{bytes(t.done)} / {bytes(t.total)} {rate(t.done, t.startedMs) ? `· ${rate(t.done, t.startedMs)}` : ""}</span>
            </div>
            <div class="bar"><div style="width:{pct}%"></div></div>
          </div>
        </div>
      {/each}
      {#if transfers.length > 12}<div class="more faint">+ {transfers.length - 12} more</div>{/if}
    </div>
  {/if}

  {#if pairs.length}
    <h2 class="section-title">Sync pairs</h2>
    <div class="pairs">
      {#each pairs as p (p.id)}<PairCard pair={p} />{/each}
    </div>
  {/if}

  {#if app.activity.length}
    <h2 class="section-title row">Recent activity <span class="spacer"></span><button class="btn sm ghost" onclick={() => (app.view = "activity")}>View all <Icon name="chevron" size={13} /></button></h2>
    <div class="card list">
      {#each app.activity.slice(0, 8) as a (a.id)}<ActivityRow item={a} />{/each}
    </div>
  {/if}

  {#if !app.config?.accounts.length}
    <div class="card onboarding">
      <Icon name="key" size={22} />
      <div class="grow">
        <h3>Connect a Google account</h3>
        <p class="muted">Add your Google account to start syncing. You'll need a free Google API client ID; the Accounts page walks you through it.</p>
      </div>
      <button class="btn primary" onclick={() => (app.view = "accounts")}>Go to Accounts</button>
    </div>
  {/if}
</div>

<style>
  .hero {
    display: flex;
    align-items: center;
    gap: 18px;
    padding: 20px 22px;
    background: linear-gradient(135deg, color-mix(in srgb, var(--ok) 9%, var(--surface)), var(--surface) 60%);
  }
  .hero.busy {
    background: linear-gradient(135deg, color-mix(in srgb, var(--accent) 12%, var(--surface)), var(--surface) 60%);
  }
  .hero.attention {
    background: linear-gradient(135deg, color-mix(in srgb, var(--danger) 10%, var(--surface)), var(--surface) 60%);
  }
  .hero.paused {
    background: var(--surface);
  }
  .orb {
    display: grid;
    place-items: center;
    width: 54px;
    height: 54px;
    border-radius: 16px;
    background: var(--ok-soft);
    color: var(--ok);
    flex: none;
  }
  .busy .orb {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .attention .orb {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .paused .orb {
    background: var(--surface-2);
    color: var(--text-2);
  }
  .hero-text {
    flex: 1;
    min-width: 0;
  }
  .hero h1 {
    font-size: 20px;
  }
  .hero p {
    margin-top: 3px;
  }
  .hero-actions {
    display: flex;
    gap: 8px;
  }
  .confirm,
  .onboarding {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    padding: 16px 18px;
    margin-top: 14px;
  }
  .confirm {
    border-color: color-mix(in srgb, var(--warn) 50%, var(--border));
    background: color-mix(in srgb, var(--warn) 6%, var(--surface));
  }
  .confirm > :global(.icon) {
    color: var(--warn);
    margin-top: 2px;
  }
  .onboarding > :global(.icon) {
    color: var(--accent);
    margin-top: 2px;
  }
  .confirm h3,
  .onboarding h3 {
    font-size: 14.5px;
    margin-bottom: 4px;
  }
  .sample {
    margin-top: 6px;
    font-size: 11.5px;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: 12px;
    margin-top: 14px;
  }
  .stat {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 12px 14px;
  }
  .stat .k {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--text-2);
  }
  .stat .v {
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.02em;
    font-variant-numeric: tabular-nums;
  }
  .stat.bad .v {
    color: var(--danger);
  }
  .linkish {
    border: none;
    background: none;
    padding: 0;
    text-align: left;
    cursor: pointer;
  }
  .transfers {
    padding: 6px 0;
  }
  .tr {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 16px;
  }
  .tr .dir {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    background: var(--ok-soft);
    color: var(--ok);
    flex: none;
  }
  .tr .dir.up {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .tr-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .tr .row {
    gap: 8px;
  }
  .name {
    font-weight: 500;
    flex: none;
    max-width: 50%;
  }
  .small {
    font-size: 12px;
  }
  .nowrap {
    white-space: nowrap;
  }
  .bar {
    height: 4px;
    border-radius: 4px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--accent);
    border-radius: 4px;
    transition: width 0.3s ease;
  }
  .more {
    padding: 6px 16px 8px 54px;
    font-size: 12px;
  }
  .pairs {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(420px, 1fr));
    gap: 12px;
  }
  .list {
    padding: 4px 0;
  }
  .section-title.row {
    display: flex;
  }
</style>
