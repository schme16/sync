<script lang="ts">
  import { api, type Config, type Theme } from "../api";
  import { app, applyTheme, errText, toast } from "../store.svelte";
  import Icon from "../Icon.svelte";
  import Toggle from "../components/Toggle.svelte";
  import Segmented from "../components/Segmented.svelte";

  let c = $state(structuredClone($state.snapshot(app.config!)));

  async function patch(p: Partial<Config>) {
    try {
      app.config = await api.saveSettings(p);
      c = structuredClone($state.snapshot(app.config));
    } catch (e) {
      toast(errText(e), "error");
      c = structuredClone($state.snapshot(app.config!));
    }
  }

  function setTheme(t: Theme) {
    applyTheme(t);
    patch({ theme: t });
  }

  function num(key: keyof Config, min: number, max: number) {
    return (e: Event) => {
      const v = Math.round(Number((e.target as HTMLInputElement).value));
      if (Number.isFinite(v)) patch({ [key]: Math.min(max, Math.max(min, v)) } as Partial<Config>);
    };
  }
</script>

<div class="page">
  <div class="page-head">
    <div>
      <h1>Settings</h1>
      <p>Changes are saved as you make them.</p>
    </div>
  </div>

  <section>
    <h2 class="section-title">Appearance</h2>
    <div class="card group">
      <div class="item">
        <div><div class="label">Theme</div><div class="hint">Auto follows your system's light or dark mode.</div></div>
        <Segmented
          bind:value={c.theme}
          onchange={setTheme}
          options={[
            { value: "auto", label: "Auto", icon: "auto" },
            { value: "light", label: "Light", icon: "sun" },
            { value: "dark", label: "Dark", icon: "moon" },
          ]} />
      </div>
    </div>
  </section>

  <section>
    <h2 class="section-title">Startup</h2>
    <div class="card group">
      <div class="item">
        <div><div class="label">Launch when I sign in</div><div class="hint">Start Sync quietly in the tray when you log in.</div></div>
        <Toggle bind:checked={c.launchAtStartup} onchange={(v) => patch({ launchAtStartup: v })} label="Launch at sign-in" />
      </div>
      <div class="item">
        <div><div class="label">Start minimized</div><div class="hint">Don't open this window on start; Sync keeps running in the tray.</div></div>
        <Toggle bind:checked={c.startMinimized} onchange={(v) => patch({ startMinimized: v })} label="Start minimized" />
      </div>
    </div>
  </section>

  <section>
    <h2 class="section-title">Safety</h2>
    <div class="card group">
      <div class="item">
        <div><div class="label">Use the recycle bin</div><div class="hint">Files removed by sync go to the recycle bin / trash instead of being deleted permanently. Drive deletions always go to the Drive trash.</div></div>
        <Toggle bind:checked={c.useTrash} onchange={(v) => patch({ useTrash: v })} label="Use recycle bin" />
      </div>
      <div class="item">
        <div><div class="label">Ask before large deletions</div><div class="hint">Hold deletions and ask first when one sync would remove more than this many items. 0 turns this off.</div></div>
        <input class="input num" type="number" min="0" value={c.deleteSafetyThreshold} onchange={num("deleteSafetyThreshold", 0, 1_000_000)} />
      </div>
    </div>
  </section>

  <section>
    <h2 class="section-title">Performance</h2>
    <div class="card group">
      <div class="item">
        <div><div class="label">Parallel transfers</div><div class="hint">Files uploaded or downloaded at the same time. Higher helps with many small files.</div></div>
        <input class="input num" type="number" min="1" max="32" value={c.maxConcurrentTransfers} onchange={num("maxConcurrentTransfers", 1, 32)} />
      </div>
      <div class="item">
        <div><div class="label">Check Drive for changes every</div><div class="hint">Seconds between change-feed polls. A poll with no changes is a single tiny request.</div></div>
        <div class="row unit"><input class="input num" type="number" min="5" max="3600" value={c.remotePollSecs} onchange={num("remotePollSecs", 5, 3600)} /><span class="faint">sec</span></div>
      </div>
      <div class="item">
        <div><div class="label">Wait for changes to settle</div><div class="hint">Debounce after a local change, so a burst like a git checkout is synced in one pass.</div></div>
        <div class="row unit"><input class="input num" type="number" min="200" max="30000" step="100" value={c.debounceMs} onchange={num("debounceMs", 200, 30000)} /><span class="faint">ms</span></div>
      </div>
      <div class="item">
        <div><div class="label">Full safety rescan every</div><div class="hint">Change notifications catch almost everything; a periodic rescan catches the rest.</div></div>
        <div class="row unit"><input class="input num" type="number" min="5" max="1440" value={c.fullRescanMins} onchange={num("fullRescanMins", 5, 1440)} /><span class="faint">min</span></div>
      </div>
    </div>
  </section>

  {#if app.info}
    <section>
      <h2 class="section-title">About</h2>
      <div class="card group">
        <div class="item">
          <div><div class="label">Sync {app.info.version}</div><div class="hint">{app.info.os === "windows" ? "Windows" : app.info.os === "linux" ? "Linux" : app.info.os}</div></div>
          <button class="btn" onclick={() => api.openLogDir()}><Icon name="logs" size={15} /> Open logs</button>
        </div>
        <div class="item paths">
          <div class="label">Locations</div>
          <dl class="mono selectable">
            <dt>Config</dt><dd>{app.info.configDir}</dd>
            <dt>State</dt><dd>{app.info.dataDir}</dd>
            <dt>Logs</dt><dd>{app.info.logDir}</dd>
          </dl>
        </div>
      </div>
    </section>
  {/if}
</div>

<style>
  section {
    max-width: 860px;
  }
  .group {
    padding: 0 18px;
  }
  .item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 24px;
    padding: 14px 0;
  }
  .item + .item {
    border-top: 1px solid var(--border);
  }
  .item .hint {
    margin-top: 2px;
    max-width: 520px;
  }
  .num {
    width: 96px;
    text-align: right;
  }
  .unit {
    gap: 8px;
  }
  .unit span {
    width: 26px;
  }
  .paths {
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
  }
  dl {
    display: grid;
    grid-template-columns: 70px 1fr;
    gap: 5px 10px;
    margin: 0;
    font-size: 12px;
  }
  dt {
    color: var(--text-3);
  }
  dd {
    margin: 0;
    color: var(--text-2);
    word-break: break-all;
  }
</style>
