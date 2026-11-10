<script lang="ts" generics="T extends string">
  import Icon from "../Icon.svelte";

  let {
    value = $bindable(),
    options,
    onchange,
  }: { value: T; options: { value: T; label: string; icon?: string }[]; onchange?: (v: T) => void } = $props();
</script>

<div class="seg" role="radiogroup">
  {#each options as o (o.value)}
    <button
      type="button"
      role="radio"
      aria-checked={value === o.value}
      class:active={value === o.value}
      onclick={() => {
        value = o.value;
        onchange?.(o.value);
      }}>
      {#if o.icon}<Icon name={o.icon} size={15} />{/if}
      {o.label}
    </button>
  {/each}
</div>

<style>
  .seg {
    display: inline-flex;
    padding: 3px;
    gap: 2px;
    border-radius: 9px;
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px;
    border-radius: 7px;
    border: none;
    background: transparent;
    color: var(--text-2);
    font-weight: 500;
    cursor: pointer;
  }
  button:hover {
    color: var(--text);
  }
  button.active {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm), 0 0 0 1px var(--border);
  }
  button:focus-visible {
    outline: none;
    box-shadow: var(--focus);
  }
</style>
