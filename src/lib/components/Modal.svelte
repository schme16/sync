<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "../Icon.svelte";

  let {
    title,
    subtitle = "",
    width = 560,
    onclose,
    children,
    footer,
  }: { title: string; subtitle?: string; width?: number; onclose: () => void; children: Snippet; footer?: Snippet } = $props();

  function key(e: KeyboardEvent) {
    if (e.key === "Escape") onclose();
  }
</script>

<svelte:window onkeydown={key} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onmousedown={(e) => e.target === e.currentTarget && onclose()}>
  <div class="modal" style="width:min({width}px, calc(100vw - 48px))" role="dialog" aria-modal="true" aria-label={title}>
    <header>
      <div>
        <h2>{title}</h2>
        {#if subtitle}<p class="muted">{subtitle}</p>{/if}
      </div>
      <button class="icon-btn" onclick={onclose} aria-label="Close"><Icon name="x" /></button>
    </header>
    <div class="body">{@render children()}</div>
    {#if footer}<footer>{@render footer()}</footer>{/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    background: rgba(8, 10, 14, 0.42);
    display: grid;
    place-items: center;
    animation: fade 0.12s ease-out;
  }
  .modal {
    max-height: calc(100vh - 48px);
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: var(--shadow-lg);
    animation: pop 0.14s ease-out;
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 18px 20px 12px;
  }
  header h2 {
    font-size: 17px;
  }
  header p {
    margin-top: 3px;
  }
  .body {
    padding: 4px 20px 18px;
    overflow-y: auto;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 14px 20px;
    border-top: 1px solid var(--border);
    background: var(--surface-2);
    border-radius: 0 0 14px 14px;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes pop {
    from {
      transform: translateY(6px) scale(0.985);
      opacity: 0;
    }
  }
</style>
