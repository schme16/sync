<script lang="ts">
  import { api, openUrl } from "../api";
  import { app, errText, reloadConfig, toast } from "../store.svelte";
  import Icon from "../Icon.svelte";

  let signingIn = $state(false);
  let clientId = $state(app.config!.googleClientId);
  let clientSecret = $state(app.config!.googleClientSecret);
  let showSecret = $state(false);
  let savingCreds = $state(false);
  let hasCreds = $derived(!!app.config!.googleClientId || !!app.info?.hasBuiltinCredentials);
  let guideOpen = $state(!app.config!.googleClientId && !app.info?.hasBuiltinCredentials);

  async function signIn() {
    signingIn = true;
    try {
      const a = await api.addAccount();
      await reloadConfig();
      toast(`Connected ${a.email}`, "ok");
    } catch (e) {
      toast(errText(e), "error");
    }
    signingIn = false;
  }

  async function cancel() {
    await api.cancelAddAccount();
    signingIn = false;
  }

  async function remove(id: string) {
    try {
      await api.removeAccount(id);
      await reloadConfig();
      toast("Account removed", "ok");
    } catch (e) {
      toast(errText(e), "error");
    }
  }

  async function saveCreds() {
    savingCreds = true;
    try {
      app.config = await api.saveSettings({ googleClientId: clientId.trim(), googleClientSecret: clientSecret.trim() });
      toast("Google API credentials saved", "ok");
      guideOpen = false;
    } catch (e) {
      toast(errText(e), "error");
    }
    savingCreds = false;
  }
</script>

<div class="page">
  <div class="page-head">
    <div>
      <h1>Accounts</h1>
      <p>Google accounts Sync can access. Sign-in happens in your browser; tokens are kept in the system keychain.</p>
    </div>
    <span class="spacer"></span>
    {#if signingIn}
      <button class="btn" onclick={cancel}>Cancel sign-in</button>
    {/if}
    <button class="btn primary" disabled={!hasCreds || signingIn} onclick={signIn}>
      {#if signingIn}<Icon name="sync" size={15} class="spin" /> Waiting for browser…{:else}<Icon name="plus" size={15} /> Add Google account{/if}
    </button>
  </div>

  {#if app.config!.accounts.length}
    <div class="card list">
      {#each app.config!.accounts as a (a.id)}
        {@const used = app.config!.pairs.filter((p) => p.accountId === a.id).length}
        <div class="acc">
          {#if a.photoLink}
            <img src={a.photoLink} alt="" referrerpolicy="no-referrer" />
          {:else}
            <span class="avatar">{(a.displayName || a.email).slice(0, 1).toUpperCase()}</span>
          {/if}
          <div class="grow">
            <div class="name">{a.displayName || a.email}</div>
            <div class="muted">{a.email}</div>
          </div>
          <span class="badge">{used} sync pair{used === 1 ? "" : "s"}</span>
          <button class="btn sm" onclick={signIn} title="Sign in again if access expired">Reconnect</button>
          <button class="btn sm danger" disabled={used > 0} title={used ? "Remove its sync pairs first" : ""} onclick={() => remove(a.id)}>Remove</button>
        </div>
      {/each}
    </div>
  {:else}
    <div class="card">
      <div class="empty">
        <Icon name="accounts" size={32} />
        <h3>No accounts connected</h3>
        <span>{hasCreds ? "Click “Add Google account” to sign in." : "First set up your Google API credentials below. It takes about five minutes, and you only do it once."}</span>
      </div>
    </div>
  {/if}

  <h2 class="section-title">Google API credentials</h2>
  <div class="card pad stack">
    <p class="muted">
      Google requires every desktop sync app to identify itself with an OAuth client. Using your own (free) client keeps your data between
      you and Google and gives you your own API quota.
      {#if app.info?.hasBuiltinCredentials}This build includes default credentials; fill these in only to override them.{/if}
    </p>

    <button class="guide-toggle" onclick={() => (guideOpen = !guideOpen)}>
      <Icon name={guideOpen ? "chevronDown" : "chevron"} size={15} /> How to create a client ID
    </button>
    {#if guideOpen}
      <ol class="guide">
        <li>Open the <button class="lnk" onclick={() => openUrl("https://console.cloud.google.com/projectcreate")}>Google Cloud console</button> and create a project (any name).</li>
        <li>Enable the <button class="lnk" onclick={() => openUrl("https://console.cloud.google.com/apis/library/drive.googleapis.com")}>Google Drive API</button> for it.</li>
        <li>Under <button class="lnk" onclick={() => openUrl("https://console.cloud.google.com/auth/branding")}>Google Auth Platform → Branding</button>, set up the consent screen (External, your email). Then under <em>Audience</em>, add yourself as a test user, or publish the app.</li>
        <li>Under <button class="lnk" onclick={() => openUrl("https://console.cloud.google.com/auth/clients")}>Clients</button>, create a client of type <strong>Desktop app</strong>.</li>
        <li>Paste the client ID and client secret below and save.</li>
      </ol>
    {/if}

    <div class="grid2">
      <div class="field">
        <label for="cid">Client ID</label>
        <input id="cid" class="input mono" placeholder="xxxxxxxx.apps.googleusercontent.com" bind:value={clientId} />
      </div>
      <div class="field">
        <label for="csec">Client secret</label>
        <div class="row secret">
          <input id="csec" class="input mono" type={showSecret ? "text" : "password"} placeholder="GOCSPX-…" bind:value={clientSecret} />
          <button class="icon-btn" aria-label="Show secret" onclick={() => (showSecret = !showSecret)}><Icon name={showSecret ? "eyeOff" : "eye"} size={16} /></button>
        </div>
      </div>
    </div>
    <div class="row">
      <span class="spacer"></span>
      <button class="btn primary" disabled={savingCreds || (clientId === app.config!.googleClientId && clientSecret === app.config!.googleClientSecret)} onclick={saveCreds}>Save credentials</button>
    </div>
  </div>
</div>

<style>
  .list {
    padding: 4px 0;
  }
  .acc {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
  }
  .acc + .acc {
    border-top: 1px solid var(--border);
  }
  img,
  .avatar {
    width: 38px;
    height: 38px;
    border-radius: 50%;
    flex: none;
  }
  .avatar {
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .name {
    font-weight: 600;
  }
  .pad {
    padding: 18px;
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .secret {
    gap: 4px;
  }
  .guide-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: none;
    background: none;
    color: var(--accent);
    font-weight: 600;
    padding: 0;
    cursor: pointer;
    align-self: flex-start;
  }
  .guide {
    margin: 0;
    padding: 12px 16px 12px 34px;
    border-radius: var(--radius-sm);
    background: var(--surface-2);
    display: flex;
    flex-direction: column;
    gap: 8px;
    color: var(--text-2);
  }
  .lnk {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    font-weight: 500;
  }
  .lnk:hover {
    text-decoration: underline;
  }
  .guide strong,
  .guide em {
    color: var(--text);
  }
</style>
