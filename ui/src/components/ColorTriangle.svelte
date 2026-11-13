<script lang="ts">
  // HSV triangle colour picker: a hue ring around a triangle whose corners are
  // the pure hue, white and black. Drag on the ring for hue, in the triangle
  // for saturation and brightness.
  import { hexToRgb, hsvToRgb, isHex, rgbToHex, rgbToHsv } from '../lib/themes.svelte';

  let { value = $bindable('#5B9DFF'), size = 232, onchange }:
    { value?: string; size?: number; onchange?: (hex: string) => void } = $props();

  let canvas = $state<HTMLCanvasElement | undefined>();
  // h, s, v are the picker's own state: hue survives passing through grey or black
  let h = $state(0), s = $state(0), v = $state(0);
  let hexText = $state('');
  let last = '';

  const R = $derived(size / 2);
  const ringIn = $derived(R * 0.8);
  const triR = $derived(ringIn - 5);

  // an outside change to `value` (another swatch selected, a preset loaded)
  $effect(() => {
    if (value === last || !isHex(value)) return;
    last = value;
    const [nh, ns, nv] = rgbToHsv(hexToRgb(value));
    if (ns > 0 && nv > 0) h = nh;
    s = ns; v = nv;
    hexText = value.toUpperCase();
  });

  function commit() {
    const hex = rgbToHex(hsvToRgb(h, s, v));
    last = hex;
    hexText = hex;
    value = hex;
    onchange?.(hex);
  }

  const rad = (deg: number) => (deg * Math.PI) / 180;
  /** Triangle corners: A = pure hue (points at the hue on the ring), B = white, C = black. */
  function corners(hue: number) {
    const pt = (deg: number) => [R + triR * Math.cos(rad(deg)), R + triR * Math.sin(rad(deg))] as const;
    return { A: pt(hue), B: pt(hue + 120), C: pt(hue + 240) };
  }
  /** Barycentric weights of (x, y): [hue, white, black]. */
  function weights(x: number, y: number, hue: number) {
    const { A, B, C } = corners(hue);
    const d = (B[1] - C[1]) * (A[0] - C[0]) + (C[0] - B[0]) * (A[1] - C[1]);
    const wa = ((B[1] - C[1]) * (x - C[0]) + (C[0] - B[0]) * (y - C[1])) / d;
    const wb = ((C[1] - A[1]) * (x - C[0]) + (A[0] - C[0]) * (y - C[1])) / d;
    return [wa, wb, 1 - wa - wb] as const;
  }

  $effect(() => {
    const cv = canvas;
    if (!cv) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const px = Math.round(size * dpr);
    if (cv.width !== px) { cv.width = px; cv.height = px; }
    const ctx = cv.getContext('2d')!;
    const img = ctx.createImageData(px, px);
    const data = img.data;
    const hue = h;
    const pure = hsvToRgb(hue, 1, 1);
    const aa = 1 / dpr; // edge softening, in CSS px
    for (let j = 0; j < px; j++) {
      for (let i = 0; i < px; i++) {
        const x = (i + 0.5) / dpr, y = (j + 0.5) / dpr;
        const dx = x - R, dy = y - R, dist = Math.hypot(dx, dy);
        const o = (j * px + i) * 4;
        if (dist >= ringIn - aa && dist <= R) {
          const c = hsvToRgb((Math.atan2(dy, dx) * 180) / Math.PI, 1, 1);
          const edge = Math.min(1, (R - dist) / aa, (dist - ringIn + aa) / aa);
          data[o] = c[0]; data[o + 1] = c[1]; data[o + 2] = c[2]; data[o + 3] = 255 * Math.max(0, edge);
        } else if (dist < triR + aa) {
          const [wa, wb, wc] = weights(x, y, hue);
          const m = Math.min(wa, wb, wc);
          if (m > -0.012) {
            // colour = hue*wa + white*wb + black*wc
            const a = Math.max(0, wa), b = Math.max(0, wb);
            data[o] = pure[0] * a + 255 * b; data[o + 1] = pure[1] * a + 255 * b; data[o + 2] = pure[2] * a + 255 * b;
            data[o + 3] = 255 * Math.min(1, (m + 0.012) / 0.012);
          }
        }
      }
    }
    ctx.putImageData(img, 0, 0);
  });

  // marker positions
  const huePos = $derived.by(() => {
    const r = (R + ringIn) / 2;
    return [R + r * Math.cos(rad(h)), R + r * Math.sin(rad(h))];
  });
  const svPos = $derived.by(() => {
    const { A, B, C } = corners(h);
    const wa = s * v, wb = (1 - s) * v, wc = 1 - v;
    return [A[0] * wa + B[0] * wb + C[0] * wc, A[1] * wa + B[1] * wb + C[1] * wc];
  });

  let mode: 'ring' | 'tri' | null = null;
  function at(e: PointerEvent) {
    const r = canvas!.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * size, ((e.clientY - r.top) / r.height) * size];
  }
  function pick(e: PointerEvent) {
    const [x, y] = at(e);
    if (mode === 'ring') {
      h = ((Math.atan2(y - R, x - R) * 180) / Math.PI + 360) % 360;
    } else if (mode === 'tri') {
      // clamp to the triangle, then read saturation and brightness off the weights
      let [wa, wb, wc] = weights(x, y, h).map((w) => Math.max(0, w));
      const sum = wa + wb + wc || 1;
      wa /= sum; wb /= sum; wc /= sum;
      v = wa + wb;
      s = v > 0.0001 ? wa / v : s;
    }
    commit();
  }
  function down(e: PointerEvent) {
    if (e.button !== 0) return;
    const [x, y] = at(e);
    const dist = Math.hypot(x - R, y - R);
    mode = dist >= ringIn ? (dist <= R + 4 ? 'ring' : null) : 'tri';
    if (!mode) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    pick(e);
  }
  const move = (e: PointerEvent) => { if (mode) pick(e); };
  const up = () => { mode = null; };

  function typedHex() {
    let t = hexText.trim();
    if (!t.startsWith('#')) t = '#' + t;
    if (/^#[0-9a-fA-F]{3}$/.test(t)) t = '#' + [...t.slice(1)].map((c) => c + c).join('');
    if (!isHex(t)) { hexText = last; return; }
    const [nh, ns, nv] = rgbToHsv(hexToRgb(t));
    if (ns > 0 && nv > 0) h = nh;
    s = ns; v = nv;
    commit();
  }
  function setHsv(which: 'h' | 's' | 'v', raw: string) {
    const n = Number(raw);
    if (!isFinite(n)) return;
    if (which === 'h') h = ((n % 360) + 360) % 360;
    else if (which === 's') s = Math.min(1, Math.max(0, n / 100));
    else v = Math.min(1, Math.max(0, n / 100));
    commit();
  }
  /** Arrow keys nudge the colour: left/right hue, up/down brightness, with Shift saturation. */
  function key(e: KeyboardEvent) {
    const step = e.altKey ? 10 : 1;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      const d = e.key === 'ArrowRight' ? step : -step;
      if (e.shiftKey) s = Math.min(1, Math.max(0, s + d / 100)); else h = (h + d + 360) % 360;
    } else if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      v = Math.min(1, Math.max(0, v + (e.key === 'ArrowUp' ? step : -step) / 100));
    } else return;
    e.preventDefault();
    commit();
  }
</script>

<div class="picker" style:width="{size}px">
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div class="wheel" style:width="{size}px" style:height="{size}px" role="application" tabindex="0"
    aria-label="Colour: drag the ring for hue and the triangle for saturation and brightness. Arrow keys adjust hue and brightness; with Shift, saturation."
    onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up} onkeydown={key}>
    <canvas bind:this={canvas} style:width="{size}px" style:height="{size}px"></canvas>
    <span class="mark ring" style:left="{huePos[0]}px" style:top="{huePos[1]}px"></span>
    <span class="mark" style:left="{svPos[0]}px" style:top="{svPos[1]}px" style:background={value}></span>
  </div>
  <div class="fields">
    <span class="chip" style:background={value}></span>
    <input class="input mono hex" aria-label="Hex colour" bind:value={hexText} onchange={typedHex} spellcheck="false" maxlength="7" />
  </div>
  <div class="fields hsv">
    <label>H<input class="input mono" inputmode="numeric" value={Math.round(h)} onchange={(e) => setHsv('h', e.currentTarget.value)} aria-label="Hue, degrees" /></label>
    <label>S<input class="input mono" inputmode="numeric" value={Math.round(s * 100)} onchange={(e) => setHsv('s', e.currentTarget.value)} aria-label="Saturation, percent" /></label>
    <label>V<input class="input mono" inputmode="numeric" value={Math.round(v * 100)} onchange={(e) => setHsv('v', e.currentTarget.value)} aria-label="Brightness, percent" /></label>
  </div>
</div>

<style>
  .picker { display: flex; flex-direction: column; gap: 10px; }
  .wheel { position: relative; touch-action: none; cursor: crosshair; border-radius: 50%; }
  .wheel:focus-visible { outline-offset: 4px; }
  canvas { display: block; }
  .mark {
    position: absolute; width: 14px; height: 14px; margin: -7px 0 0 -7px; border-radius: 50%;
    border: 2px solid #fff; box-shadow: 0 0 0 1.5px rgba(0,0,0,.65), 0 1px 4px rgba(0,0,0,.5); pointer-events: none;
  }
  .mark.ring { background: transparent; }
  .fields { display: flex; align-items: center; gap: 8px; }
  .chip { width: 36px; height: 32px; border-radius: var(--r-md); border: 1px solid var(--border); flex: none; }
  .hex { flex: 1; height: 32px; text-transform: uppercase; }
  .hsv label { flex: 1; display: flex; align-items: center; gap: 5px; font-size: var(--fs-sm); color: var(--text-muted); min-width: 0; }
  .hsv input { width: 100%; height: 30px; padding: 0 6px; text-align: right; }
</style>
