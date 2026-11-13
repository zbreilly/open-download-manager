<script lang="ts">
  import type { Segment, DownloadStatus } from '../lib/ipc';
  import { statusColor } from '../lib/format';
  let { segments, status, total, height = 22, head = false }:
    { segments: Segment[]; status: DownloadStatus; total: number | null; height?: number; head?: boolean } = $props();

  // one cell per segment, ordered by offset; unknown size => single cell
  const cells = $derived.by(() => {
    const segs = [...(segments ?? [])].sort((a, b) => a.start - b.start);
    if (!segs.length) return [{ frac: status === 'complete' ? 1 : 0, done: status === 'complete', active: false, flex: 1 }];
    const tot = total ?? segs.reduce((m, s) => Math.max(m, s.start + s.received), 1);
    return segs.map((s) => {
      const known = s.end < 1.8e19;
      const size = known ? Math.max(1, s.end - s.start) : Math.max(1, tot - s.start);
      const frac = status === 'complete' ? 1 : Math.min(1, s.received / size);
      return {
        frac,
        done: s.state === 'done' || frac >= 1,
        active: s.state === 'receiving',
        flex: Math.max(size / Math.max(1, tot), 0.015),
      };
    });
  });
  const color = $derived(statusColor(status));
</script>

<div class="map" style:height="{height}px">
  {#each cells as c}
    <div class="cell" style:flex-grow={c.flex}>
      <div class="fill" style:width="{c.frac * 100}%" style:background-color={color} style:opacity={c.done ? 0.55 : 1}>
        {#if head && c.active && c.frac < 1}<span class="head"></span>{/if}
      </div>
    </div>
  {/each}
</div>

<style>
  .map { display: flex; gap: 3px; width: 100%; }
  .cell { flex-basis: 0; min-width: 4px; height: 100%; border-radius: 4px; background: var(--track); overflow: hidden; box-shadow: inset 0 1px 2px rgba(0,0,0,.18); }
  .fill { height: 100%; position: relative; background-image: linear-gradient(180deg, rgba(255,255,255,.28), rgba(255,255,255,0) 60%); }
  .head { position: absolute; right: 0; top: 0; bottom: 0; width: 10px; background: linear-gradient(90deg, transparent, #fff); opacity: .55; }
</style>
