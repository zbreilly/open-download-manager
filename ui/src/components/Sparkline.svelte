<script lang="ts" module>
  let seq = 0;
</script>

<script lang="ts">
  import { speed } from '../lib/format';
  let { samples, height = 64, caption = true, label = 'Speed, last 60 s' }:
    { samples: number[]; height?: number; caption?: boolean; label?: string } = $props();
  const W = 400;
  const gid = `spark-${++seq}`;
  const pts = $derived.by(() => {
    const s = (samples ?? []).slice(-60);
    const peak = Math.max(1, ...s);
    const n = 60;
    const off = n - s.length;
    const xy = s.map((v, i) => [((off + i) / (n - 1)) * W, height - 4 - (v / peak) * (height - 10)]);
    const line = xy.map((p) => p.join(',')).join(' ');
    const area = xy.length ? `${xy[0][0]},${height} ${line} ${xy[xy.length - 1][0]},${height}` : '';
    return { line, area, peak: Math.max(0, ...s) };
  });
</script>

<div>
  {#if caption}
    <div class="cap"><span>{label}</span><span class="mono">peak {speed(pts.peak)}</span></div>
  {/if}
  <svg viewBox="0 0 {W} {height}" preserveAspectRatio="none" style:height="{height}px" aria-hidden="true">
    <defs>
      <linearGradient id={gid} x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="var(--accent)" stop-opacity=".45" />
        <stop offset="1" stop-color="var(--violet)" stop-opacity=".04" />
      </linearGradient>
    </defs>
    {#if pts.area}<polygon points={pts.area} fill="url(#{gid})" />{/if}
    {#if pts.line}<polyline points={pts.line} fill="none" stroke="var(--accent)" stroke-width="1.8" vector-effect="non-scaling-stroke" />{/if}
  </svg>
</div>

<style>
  .cap { display: flex; justify-content: space-between; font-size: var(--fs-sm); color: var(--text-muted); margin-bottom: 6px; }
  svg { width: 100%; display: block; border-radius: 6px; background: var(--surface-sunken); border: 1px solid var(--border-subtle); }
</style>
