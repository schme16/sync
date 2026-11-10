<script lang="ts">
  import { api, pickDirectory, type PairConfig, type SyncMode } from "../api";
  import { app, errText, reloadConfig, toast } from "../store.svelte";
  import Icon from "../Icon.svelte";
  import Modal from "./Modal.svelte";
  import Toggle from "./Toggle.svelte";
  import RuleEditor from "./RuleEditor.svelte";
  import DriveFolderPicker from "./DriveFolderPicker.svelte";
  import FolderTree from "./FolderTree.svelte";

  let { id, onclose }: { id: string; onclose: () => void } = $props();

  const isNew = id === "new";
  const existing = app.config!.pairs.find((p) => p.id === id);
  let pair = $state<PairConfig>(
    existing
      ? structuredClone($state.snapshot(existing))
      : {
          id: "",
          name: "",
          accountId: app.config!.accounts[0]?.id ?? "",
          localPath: "",
          remoteFolderId: "",
          remotePathDisplay: "",
          driveId: null,
          mode: "twoWay",
          propagateDeletes: true,
          paused: false,
          useGlobalIgnore: true,
          ignore: "",
          mounts: [],
        },
  );

  type Tab = "general" | "rules" | "selective" | "mounts";
  let tab = $state<Tab>("general");
  let picking = $state(false);
  let saving = $state(false);
  let confirmDelete = $state(false);

  const modes: { value: SyncMode; title: string; desc: string; icon: string }[] = [
    { value: "twoWay", title: "Two-way", desc: "Changes on either side are mirrored to the other.", icon: "twoWay" },
    { value: "localToRemote", title: "Upload only", desc: "This computer → Drive. Drive-side edits are never downloaded.", icon: "up" },
    { value: "remoteToLocal", title: "Download only", desc: "Drive → this computer. Local edits are never uploaded.", icon: "down" },
  ];

  async function browseLocal() {
    const p = await pickDirectory("Choose the local folder to sync");
    if (p) {
      pair.localPath = p;
      if (!pair.name) pair.name = p.split(/[\\/]/).filter(Boolean).pop() ?? "";
    }
  }

  async function addMount() {
    const p = await pickDirectory("Choose an external folder to include");
    if (p) pair.mounts.push({ localPath: p, relPath: p.split(/[\\/]/).filter(Boolean).pop() ?? "" });
  }

  async function save() {
    saving = true;
    try {
      const saved = isNew ? await api.addPair($state.snapshot(pair)) : await api.updatePair($state.snapshot(pair));
      await reloadConfig();
      toast(isNew ? `"${saved.name}" added — initial sync started` : "Changes saved", "ok");
      onclose();
    } catch (e) {
      toast(errText(e), "error");
      if (String(e).includes("Drive folder")) tab = "general";
    }
    saving = false;
  }

  async function remove() {
    try {
      await api.removePair(pair.id);
      await reloadConfig();
      toast(`"${pair.name}" removed. Your files were not touched.`, "ok");
      onclose();
    } catch (e) {
      toast(errText(e), "error");
    }
  }

  let account = $derived(app.config!.accounts.find((a) => a.id === pair.accountId));
</script>

<Modal title={isNew ? "New sync pair" : `Edit “${existing?.name}”`} subtitle="Connect a folder on this computer to a folder in Google Drive." width={720} {onclose}>
  <div class="tabs">
    <button class:active={tab === "general"} onclick={() => (tab = "general")}>General</button>
    <button class:active={tab === "rules"} onclick={() => (tab = "rules")}>Exclusions</button>
    <button class:active={tab === "selective"} onclick={() => (tab = "selective")} disabled={isNew} title={isNew ? "Available after the pair is created" : ""}>Selective sync</button>
    <button class:active={tab === "mounts"} onclick={() => (tab = "mounts")}>External folders {#if pair.mounts.length}<span class="count">{pair.mounts.length}</span>{/if}</button>
  </div>

  {#if tab === "general"}
    <div class="stack">
      <div class="grid2">
        <div class="field">
          <label for="pname">Name</label>
          <input id="pname" class="input" placeholder="e.g. Projects" bind:value={pair.name} />
        </div>
        <div class="field">
          <label for="pacc">Google account</label>
          <select id="pacc" class="select" bind:value={pair.accountId} disabled={!isNew && !!pair.remoteFolderId}>
            {#each app.config!.accounts as a (a.id)}<option value={a.id}>{a.email}</option>{/each}
          </select>
        </div>
      </div>

      <div class="ends">
        <div class="end">
          <div class="end-head"><Icon name="laptop" size={16} /> This computer</div>
          <div class="row">
            <input class="input mono" placeholder="C:\Users\you\Documents" bind:value={pair.localPath} />
            <button class="btn" onclick={browseLocal}>Browse…</button>
          </div>
        </div>
        <div class="link"><Icon name={modes.find((m) => m.value === pair.mode)!.icon} size={18} /></div>
        <div class="end">
          <div class="end-head"><Icon name="cloud" size={16} /> Google Drive</div>
          <div class="row">
            <div class="input fake mono ellipsis" class:placeholder={!pair.remotePathDisplay}>{pair.remotePathDisplay || "No folder chosen"}</div>
            <button class="btn" disabled={!account} onclick={() => (picking = true)}>Choose…</button>
          </div>
        </div>
      </div>

      <div class="field">
        <span class="label">Sync direction</span>
        <div class="modes">
          {#each modes as m (m.value)}
            <button class="mode" class:active={pair.mode === m.value} onclick={() => (pair.mode = m.value)}>
              <span class="mi"><Icon name={m.icon} size={18} /></span>
              <strong>{m.title}</strong>
              <span class="hint">{m.desc}</span>
            </button>
          {/each}
        </div>
      </div>

      <div class="setting">
        <div>
          <div class="label">Propagate deletions</div>
          <div class="hint">
            {#if pair.mode === "twoWay"}When off, a file deleted on one side is restored from the other instead.{:else}When off, deleting a source file leaves the copy on the other side alone.{/if}
            Deleted files always go to the recycle bin / Drive trash.
          </div>
        </div>
        <Toggle bind:checked={pair.propagateDeletes} label="Propagate deletions" />
      </div>
    </div>
  {:else if tab === "rules"}
    <div class="stack">
      <div class="setting">
        <div>
          <div class="label">Use global exclusions</div>
          <div class="hint">Apply the shared rules from the Exclusions page (node_modules, OS junk files…). Rules below take priority.</div>
        </div>
        <Toggle bind:checked={pair.useGlobalIgnore} label="Use global exclusions" />
      </div>
      <div class="field">
        <span class="label">Rules for this pair</span>
        <span class="hint">Same syntax as <code>.gitignore</code>: <code>*.log</code>, <code>build/</code>, <code>/top-level-only</code>, <code>**/cache/</code>, and <code>!pattern</code> to re-include.</span>
        <RuleEditor bind:value={pair.ignore} useGlobal={pair.useGlobalIgnore} rows={12} placeholder={"# Examples\ndist/\n*.psd\n!important.psd"} />
      </div>
    </div>
  {:else if tab === "selective"}
    <p class="hint" style="margin-bottom:10px">Untick folders you don't want on this computer or in Drive. Each choice is stored as an exclusion rule, so it is applied immediately.</p>
    <div class="tree">
      <FolderTree pairId={pair.id} onchanged={async () => {
        await reloadConfig();
        const p = app.config!.pairs.find((x) => x.id === pair.id);
        if (p) pair.ignore = p.ignore;
      }} />
    </div>
  {:else}
    <div class="stack">
      <p class="hint">
        Include folders that live elsewhere on this computer, for example on another drive. Each one appears inside the sync folder at the path you choose, and its contents sync to Drive as if they were really there. Nothing is moved or linked on disk.
      </p>
      {#each pair.mounts as m, i}
        <div class="mount card">
          <div class="field" style="flex:1.3">
            <span class="label">External folder</span>
            <div class="row">
              <input class="input mono" bind:value={m.localPath} />
              <button class="btn sm" onclick={async () => { const p = await pickDirectory("Choose external folder"); if (p) m.localPath = p; }}>Browse…</button>
            </div>
          </div>
          <Icon name="arrowRight" size={18} class="faint" />
          <div class="field" style="flex:1">
            <span class="label">Appears as</span>
            <div class="row prefix">
              <span class="mono faint">{(pair.name || "sync") + "/"}</span>
              <input class="input mono" placeholder="Notes" bind:value={m.relPath} />
            </div>
          </div>
          <button class="icon-btn" aria-label="Remove" onclick={() => pair.mounts.splice(i, 1)}><Icon name="trash" size={16} /></button>
        </div>
      {/each}
      <div><button class="btn" onclick={addMount}><Icon name="plus" size={15} /> Add external folder</button></div>
    </div>
  {/if}

  {#if picking && account}
    <DriveFolderPicker
      accountId={account.id}
      onclose={() => (picking = false)}
      onpick={(f) => {
        pair.remoteFolderId = f.id;
        pair.remotePathDisplay = f.display;
        pair.driveId = f.driveId;
        picking = false;
      }} />
  {/if}

  {#snippet footer()}
    {#if !isNew}
      {#if confirmDelete}
        <span class="confirm">Remove this pair? Files stay where they are.</span>
        <button class="btn danger-solid" onclick={remove}>Remove</button>
        <button class="btn ghost" onclick={() => (confirmDelete = false)}>Keep</button>
      {:else}
        <button class="btn danger" onclick={() => (confirmDelete = true)}><Icon name="trash" size={15} /> Remove pair</button>
      {/if}
    {/if}
    <span class="spacer"></span>
    <button class="btn" onclick={onclose}>Cancel</button>
    <button class="btn primary" disabled={saving || !pair.localPath || !pair.remoteFolderId} onclick={save}>
      {#if saving}<Icon name="sync" size={15} class="spin" />{/if}
      {isNew ? "Create & start syncing" : "Save changes"}
    </button>
  {/snippet}
</Modal>

<style>
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--border);
    margin: 0 -20px 18px;
    padding: 0 16px;
  }
  .tabs button {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: none;
    background: none;
    padding: 9px 10px 11px;
    color: var(--text-2);
    font-weight: 500;
    cursor: pointer;
  }
  .tabs button:hover:not(:disabled) {
    color: var(--text);
  }
  .tabs button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .tabs button.active {
    color: var(--text);
  }
  .tabs button.active::after {
    content: "";
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: -1px;
    height: 2px;
    border-radius: 2px;
    background: var(--accent);
  }
  .count {
    font-size: 11px;
    background: var(--accent-soft);
    color: var(--accent);
    border-radius: 999px;
    padding: 0 6px;
    font-weight: 700;
  }
  .grid2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
  }
  .ends {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: end;
    gap: 10px;
  }
  .end {
    display: flex;
    flex-direction: column;
    gap: 7px;
    min-width: 0;
  }
  .end .row {
    gap: 6px;
  }
  .end-head {
    display: flex;
    gap: 6px;
    align-items: center;
    font-weight: 600;
    font-size: 12.5px;
  }
  .link {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .fake {
    display: flex;
    align-items: center;
    background: var(--surface-2);
  }
  .fake.placeholder {
    color: var(--text-3);
  }
  .modes {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 10px;
  }
  .mode {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    padding: 12px;
    text-align: left;
    border-radius: var(--radius);
    border: 1px solid var(--border);
    background: var(--surface);
    cursor: pointer;
  }
  .mode:hover {
    border-color: var(--border-strong);
  }
  .mode.active {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
    background: var(--accent-soft);
  }
  .mi {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border-radius: 8px;
    background: var(--surface-2);
    color: var(--accent);
    margin-bottom: 4px;
  }
  .setting {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }
  .setting .hint {
    margin-top: 2px;
  }
  .tree {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 4px;
    height: 360px;
    overflow-y: auto;
  }
  .mount {
    display: flex;
    align-items: flex-end;
    gap: 12px;
    padding: 12px;
  }
  .mount :global(.faint) {
    margin-bottom: 8px;
  }
  .prefix {
    gap: 4px;
  }
  .confirm {
    align-self: center;
    font-weight: 500;
    color: var(--danger);
  }
</style>
