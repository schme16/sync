<script lang="ts">
  import { api, type IgnoreTest } from "../api";
  import Icon from "../Icon.svelte";

  let {
    value = $bindable(""),
    useGlobal = false,
    rows = 14,
    placeholder = "",
  }: { value: string; useGlobal?: boolean; rows?: number; placeholder?: string } = $props();

  let testPath = $state("");
  let testDir = $state(false);
  let result = $state<IgnoreTest | null>(null);
  let gutter: HTMLDivElement | undefined = $state();
  let timer: ReturnType<typeof setTimeout>;

  let lines = $derived(Math.max(rows, value.split("\n").length));

  $effect(() => {
    // re-test whenever the rules or the sample path change
    const v = value;
    const p = testPath.trim();
    const d = testDir;
    const g = useGlobal;
    clearTimeout(timer);
    timer = setTimeout(async () => {
      result = await api.testIgnore(v, g, p || "example.txt", d);
      if (!p) result = { ...result, result: { ignored: false, rule: null, source: null } };
    }, 180);
  });

  function sync(e: Event) {
    if (gutter) gutter.scrollTop = (e.target as HTMLTextAreaElement).scrollTop;
  }

  let errLines = $derived(new Set(result?.errors.map((e) => e.line) ?? []));
</script>

<div class="editor">
  <div class="gutter mono" bind:this={gutter} aria-hidden="true">
    {#each Array(lines) as _, i}<div class:err={errLines.has(i + 1)}>{i + 1}</div>{/each}
  </div>
  <textarea class="mono" {rows} spellcheck="false" bind:value {placeholder} onscroll={sync}></textarea>
</div>

{#if result?.errors.length}
  <div class="notice danger" style="margin-top:8px">
    <Icon name="error" size={16} />
    <div>{#each result.errors as e}<div>Line {e.line}: {e.message}</div>{/each}</div>
  </div>
{/if}

<div class="tester">
  <Icon name="search" size={15} />
  <input class="input mono" placeholder="Test a path, e.g. web/node_modules/react/index.js" bind:value={testPath} />
  <label class="row dir"><input type="checkbox" bind:checked={testDir} /> Folder</label>
  {#if testPath.trim() && result}
    {#if result.result.ignored}
      <span class="badge danger">Excluded{result.result.rule ? ` by ${result.result.rule}` : ""}{result.result.source === "global" ? " (global)" : ""}</span>
    {:else}
      <span class="badge ok">Synced{result.result.rule ? ` (re-included by ${result.result.rule})` : ""}</span>
    {/if}
  {/if}
</div>

<style>
  .editor {
    display: flex;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--surface);
    overflow: hidden;
  }
  .editor:focus-within {
    border-color: var(--accent);
    box-shadow: var(--focus);
  }
  .gutter {
    flex: none;
    width: 42px;
    padding: 10px 8px 10px 0;
    text-align: right;
    color: var(--text-3);
    background: var(--surface-2);
    border-right: 1px solid var(--border);
    overflow: hidden;
    line-height: 1.6;
    font-size: 12px;
  }
  .gutter .err {
    color: var(--danger);
    font-weight: 700;
  }
  textarea {
    flex: 1;
    border: none;
    outline: none;
    resize: vertical;
    padding: 10px 12px;
    background: transparent;
    line-height: 1.6;
    font-size: 12.5px;
    white-space: pre;
    overflow-wrap: normal;
    min-height: 120px;
  }
  .tester {
    display: flex;
    align-items: center;
    gap: 9px;
    margin-top: 10px;
    color: var(--text-2);
  }
  .tester .input {
    flex: 1;
    height: 30px;
  }
  .dir {
    gap: 5px;
    font-size: 12.5px;
    white-space: nowrap;
  }
</style>
