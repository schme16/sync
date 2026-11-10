<script lang="ts">
  import { api } from "../api";
  import { app, errText, toast } from "../store.svelte";
  import Icon from "../Icon.svelte";
  import RuleEditor from "../components/RuleEditor.svelte";

  let text = $state(app.config!.globalIgnore);
  let saving = $state(false);
  let dirty = $derived(text !== app.config!.globalIgnore);

  const presets: { label: string; line: string; why: string }[] = [
    { label: "node_modules", line: "node_modules/", why: "npm/yarn dependencies" },
    { label: ".git folders", line: ".git/", why: "keep repositories out of Drive entirely" },
    { label: "Build output", line: "dist/\nbuild/\nout/", why: "generated files" },
    { label: "Rust target", line: "target/", why: "cargo build output" },
    { label: "Python caches", line: "__pycache__/\n*.pyc\n.venv/", why: "bytecode and virtualenvs" },
    { label: "Logs", line: "*.log", why: "log files" },
    { label: "Temp files", line: "*.tmp\n*.temp\n~$*", why: "editor and Office temp files" },
    { label: "Large media", line: "*.iso\n*.vmdk\n*.vdi", why: "disk images" },
  ];

  function hasLine(l: string) {
    const lines = text.split("\n").map((x) => x.trim());
    return l.split("\n").every((x) => lines.includes(x));
  }

  function add(l: string) {
    if (hasLine(l)) return;
    text = text.replace(/\s*$/, "") + "\n" + l + "\n";
  }

  async function save() {
    saving = true;
    try {
      app.config = await api.saveSettings({ globalIgnore: text });
      toast("Global exclusions saved and applied", "ok");
    } catch (e) {
      toast(errText(e), "error");
    }
    saving = false;
  }
</script>

<div class="page">
  <div class="page-head">
    <div>
      <h1>Exclusions</h1>
      <p>Global rules apply to every sync pair that has “Use global exclusions” on. Each pair can add its own rules too.</p>
    </div>
    <span class="spacer"></span>
    <button class="btn" disabled={!dirty} onclick={() => (text = app.config!.globalIgnore)}>Discard</button>
    <button class="btn primary" disabled={!dirty || saving} onclick={save}>{#if saving}<Icon name="sync" size={15} class="spin" />{/if} Save rules</button>
  </div>

  <div class="layout">
    <div class="card pad">
      <RuleEditor bind:value={text} rows={22} />
    </div>
    <aside class="stack">
      <div class="card pad">
        <h3>Quick add</h3>
        <div class="chips">
          {#each presets as p (p.label)}
            <button class="chip" class:on={hasLine(p.line)} title={p.why} onclick={() => add(p.line)}>
              <Icon name={hasLine(p.line) ? "check" : "plus"} size={13} />{p.label}
            </button>
          {/each}
        </div>
      </div>
      <div class="card pad help">
        <h3>Syntax</h3>
        <dl>
          <dt><code>*.log</code></dt><dd>any file ending in .log, anywhere</dd>
          <dt><code>cache/</code></dt><dd>any folder named cache (and its contents)</dd>
          <dt><code>/drafts</code></dt><dd>only at the top of the sync folder</dd>
          <dt><code>docs/*.pdf</code></dt><dd>PDFs directly inside a top-level docs</dd>
          <dt><code>**/tmp/**</code></dt><dd>everything inside any tmp folder</dd>
          <dt><code>!keep.log</code></dt><dd>re-include something excluded above</dd>
          <dt><code># note</code></dt><dd>a comment</dd>
        </dl>
        <p class="hint">Excluded items are simply left alone: they are never uploaded, downloaded or deleted. On Windows, matching ignores letter case.</p>
      </div>
    </aside>
  </div>
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 300px;
    gap: 14px;
    align-items: start;
  }
  .pad {
    padding: 16px;
  }
  h3 {
    font-size: 13.5px;
    margin-bottom: 10px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 27px;
    padding: 0 10px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text-2);
    cursor: pointer;
    font-size: 12.5px;
  }
  .chip:hover {
    border-color: var(--border-strong);
    color: var(--text);
  }
  .chip.on {
    background: var(--ok-soft);
    color: var(--ok);
    border-color: transparent;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 7px 10px;
    margin: 0 0 12px;
    font-size: 12.5px;
  }
  dt {
    white-space: nowrap;
  }
  dd {
    margin: 0;
    color: var(--text-2);
  }
</style>
