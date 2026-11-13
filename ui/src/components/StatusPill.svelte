<script lang="ts">
  import type { Download } from '../lib/ipc';
  import { statusColor, statusSoft, statusLabel } from '../lib/format';
  let { d }: { d: Pick<Download, 'status' | 'scheduledAt'> } = $props();
</script>

<span class="pill" style:--c={statusColor(d.status)} style:--c-soft={statusSoft(d.status)}>
  <span class="dot" class:pulse={d.status === 'connecting'}></span>
  {statusLabel(d)}
</span>

<style>
  .pill {
    display: inline-flex; align-items: center; gap: 6px; padding: 2px 9px 2px 7px;
    border-radius: var(--r-pill); border: 1px solid var(--c-soft); background: var(--c-soft); color: var(--c);
    font-size: var(--fs-sm); font-weight: 500; white-space: nowrap;
  }
  .dot { width: 7px; height: 7px; border-radius: 50%; flex: none; background: var(--c); box-shadow: 0 0 6px var(--c); }
  .pulse { animation: p 1s ease-in-out infinite alternate; }
  @keyframes p { from { opacity: 1; } to { opacity: .35; } }
</style>
