<script lang="ts">
  import type { Download } from '../lib/ipc';
  import { statusColor, statusLabel } from '../lib/format';
  let { d }: { d: Pick<Download, 'status' | 'scheduledAt'> } = $props();
</script>

<span class="pill">
  <span class="dot" class:pulse={d.status === 'connecting'} style:background={statusColor(d.status)}></span>
  {statusLabel(d)}
</span>

<style>
  .pill {
    display: inline-flex; align-items: center; gap: 6px; padding: 2px 9px 2px 7px;
    border-radius: var(--r-pill); border: 1px solid var(--border); font-size: var(--fs-sm); white-space: nowrap;
  }
  .dot { width: 7px; height: 7px; border-radius: 50%; flex: none; }
  .pulse { animation: p 1s ease-in-out infinite alternate; }
  @keyframes p { from { opacity: 1; } to { opacity: .35; } }
</style>
