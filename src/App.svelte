<script lang="ts">
  import { api } from "./lib/api";
  import { app, init, type View } from "./lib/store.svelte";
  import Icon from "./lib/Icon.svelte";
  import Overview from "./lib/views/Overview.svelte";
  import Folders from "./lib/views/Folders.svelte";
  import Activity from "./lib/views/Activity.svelte";
  import Rules from "./lib/views/Rules.svelte";
  import Accounts from "./lib/views/Accounts.svelte";
  import Settings from "./lib/views/Settings.svelte";
  import PairEditor from "./lib/components/PairEditor.svelte";

  let error = $state("");
  init().catch((e) => (error = String(e)));

  const nav: { id: View; label: string; icon: string }[] = [
    { id: "overview", label: "Overview", icon: "overview" },
    { id: "folders", label: "Sync pairs", icon: "folder" },
    { id: "activity", label: "Activity", icon: "activity" },
    { id: "rules", label: "Exclusions", icon: "rules" },
    { id: "accounts", label: "Accounts", icon: "accounts" },
    { id: "settings", label: "Settings", icon: "settings" },
  ];

  let statuses = $derived(Object.values(app.snapshot.pairs));
  let errorCount = $derived(statuses.filter((s) => s.state === "error" || s.state === "needsConfirmation").length);
  let busy = $derived(app.snapshot.transfers.length > 0 || statuses.some((s) => ["syncing", "scanning", "starting"].includes(s.state)));
  let footer = $derived(
    app.snapshot.pausedAll
      ? { text: "Paused", icon: "pause", cls: "" }
      : errorCount
        ? { text: "Needs attention", icon: "error", cls: "danger" }
        : busy
          ? { text: "Syncing", icon: "sync", cls: "accent" }
          : { text: "Up to date", icon: "check", cls: "ok" },
  );
</script>

{#if error}
  <div class="fatal"><Icon name="error" size={28} /><h2>Sync could not start</h2><p class="mono selectable">{error}</p></div>
{:else if app.ready}
  <div class="shell">
    <aside class="sidebar">
      <div class="brand">
        <svg viewBox="0 0 1024 1024" width="26" height="26" aria-hidden="true">
          <defs><linearGradient id="lg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#4F7CFF" /><stop offset="1" stop-color="#2440C8" /></linearGradient></defs>
          <rect x="64" y="64" width="896" height="896" rx="208" fill="url(#lg)" />
          <g fill="none" stroke="#fff" stroke-width="84" stroke-linecap="round"><path d="M300 470a214 214 0 0 1 382-118" /><path d="M724 554a214 214 0 0 1-382 118" /></g>
          <path d="M742 236v190H552z" fill="#fff" /><path d="M282 788V598h190z" fill="#fff" />
        </svg>
        <span>Sync</span>
      </div>
      <nav>
        {#each nav as n (n.id)}
          <button class:active={app.view === n.id} onclick={() => (app.view = n.id)}>
            <Icon name={n.icon} size={17} />
            <span>{n.label}</span>
            {#if n.id === "folders" && errorCount}<span class="dot"></span>{/if}
          </button>
        {/each}
      </nav>
      <div class="spacer"></div>
      <div class="state {footer.cls}">
        <Icon name={footer.icon} size={15} class={footer.icon === "sync" ? "spin" : ""} />
        <span>{footer.text}</span>
        <span class="spacer"></span>
        <button
          class="icon-btn"
          title={app.snapshot.pausedAll ? "Resume all" : "Pause all"}
          aria-label={app.snapshot.pausedAll ? "Resume all" : "Pause all"}
          onclick={() => api.setPausedAll(!app.snapshot.pausedAll)}>
          <Icon name={app.snapshot.pausedAll ? "play" : "pause"} size={15} />
        </button>
      </div>
    </aside>

    <main>
      {#key app.view}
        {#if app.view === "overview"}<Overview />
        {:else if app.view === "folders"}<Folders />
        {:else if app.view === "activity"}<Activity />
        {:else if app.view === "rules"}<Rules />
        {:else if app.view === "accounts"}<Accounts />
        {:else}<Settings />{/if}
      {/key}
    </main>
  </div>

  {#if app.editingPair}
    <PairEditor id={app.editingPair} onclose={() => (app.editingPair = null)} />
  {/if}

  <div class="toasts" aria-live="polite">
    {#each app.toasts as t (t.id)}
      <div class="toast {t.kind}">
        <Icon name={t.kind === "error" ? "error" : t.kind === "ok" ? "check" : "info"} size={16} />
        <span class="selectable">{t.text}</span>
      </div>
    {/each}
  </div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: 216px minmax(0, 1fr);
    height: 100%;
  }
  .sidebar {
    display: flex;
    flex-direction: column;
    padding: 16px 10px 12px;
    background: var(--sidebar);
    border-right: 1px solid var(--border);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 2px 10px 18px;
    font-weight: 700;
    font-size: 16px;
    letter-spacing: -0.01em;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  nav button {
    position: relative;
    display: flex;
    align-items: center;
    gap: 11px;
    height: 36px;
    padding: 0 11px;
    border: none;
    border-radius: 8px;
    background: none;
    color: var(--text-2);
    font-weight: 500;
    text-align: left;
    cursor: pointer;
  }
  nav button:hover {
    background: var(--hover);
    color: var(--text);
  }
  nav button.active {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm), 0 0 0 1px var(--border);
  }
  nav button.active :global(.icon) {
    color: var(--accent);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--danger);
    margin-left: auto;
  }
  .state {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 6px 6px 11px;
    border-radius: 9px;
    background: var(--surface);
    border: 1px solid var(--border);
    font-weight: 500;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .state.ok :global(.icon:first-child) {
    color: var(--ok);
  }
  .state.accent :global(.icon:first-child) {
    color: var(--accent);
  }
  .state.danger :global(.icon:first-child) {
    color: var(--danger);
  }
  main {
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }
  .toasts {
    position: fixed;
    right: 20px;
    bottom: 20px;
    z-index: 100;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 420px;
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 11px 14px;
    border-radius: 10px;
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-lg);
    animation: slide 0.18s ease-out;
  }
  .toast.ok :global(.icon) {
    color: var(--ok);
  }
  .toast.error :global(.icon) {
    color: var(--danger);
  }
  .toast.info :global(.icon) {
    color: var(--accent);
  }
  .fatal {
    display: grid;
    place-items: center;
    align-content: center;
    gap: 10px;
    height: 100%;
    padding: 40px;
    text-align: center;
    color: var(--danger);
  }
  .fatal p {
    color: var(--text-2);
    max-width: 600px;
  }
  @keyframes slide {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
  }
</style>
