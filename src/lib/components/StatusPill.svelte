<script lang="ts">
  import type { PairState } from "../api";
  import Icon from "../Icon.svelte";

  let { state }: { state: PairState | undefined } = $props();

  const map: Record<PairState, { label: string; cls: string; icon: string; spin?: boolean }> = {
    starting: { label: "Starting", cls: "accent", icon: "sync", spin: true },
    scanning: { label: "Scanning", cls: "accent", icon: "sync", spin: true },
    syncing: { label: "Syncing", cls: "accent", icon: "sync", spin: true },
    idle: { label: "Up to date", cls: "ok", icon: "check" },
    paused: { label: "Paused", cls: "", icon: "pause" },
    offline: { label: "Offline", cls: "warn", icon: "cloud" },
    error: { label: "Needs attention", cls: "danger", icon: "error" },
    needsConfirmation: { label: "Confirm deletions", cls: "warn", icon: "shield" },
  };
  let s = $derived(map[state ?? "starting"]);
</script>

<span class="badge {s.cls}"><Icon name={s.icon} size={13} class={s.spin ? "spin" : ""} />{s.label}</span>
