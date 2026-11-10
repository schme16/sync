<script lang="ts">
  let {
    checked = $bindable(false),
    disabled = false,
    label = "",
    onchange,
  }: { checked?: boolean; disabled?: boolean; label?: string; onchange?: (v: boolean) => void } = $props();
</script>

<button
  type="button"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  class="toggle"
  class:on={checked}
  {disabled}
  onclick={() => {
    checked = !checked;
    onchange?.(checked);
  }}><span></span></button>

<style>
  .toggle {
    position: relative;
    width: 36px;
    height: 21px;
    flex: none;
    border-radius: 999px;
    border: 1px solid var(--border-strong);
    background: var(--surface-2);
    cursor: pointer;
    padding: 0;
    transition: background 0.15s, border-color 0.15s;
  }
  .toggle span {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 15px;
    height: 15px;
    border-radius: 50%;
    background: var(--text-3);
    transition: transform 0.15s, background 0.15s;
  }
  .toggle.on {
    background: var(--accent);
    border-color: var(--accent);
  }
  .toggle.on span {
    transform: translateX(15px);
    background: #fff;
  }
  .toggle:focus-visible {
    outline: none;
    box-shadow: var(--focus);
  }
  .toggle:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
