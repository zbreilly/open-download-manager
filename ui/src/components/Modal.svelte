<script lang="ts">
  import type { Snippet } from 'svelte';
  import { onMount } from 'svelte';
  let { title, children, footer, onclose, width = 420 }:
    { title: string; children: Snippet; footer: Snippet; onclose: () => void; width?: number } = $props();
  let el = $state<HTMLDivElement | undefined>();
  onMount(() => {
    el?.querySelector<HTMLElement>('input, select, textarea, button.primary, button')?.focus();
    const k = (e: KeyboardEvent) => { if (e.key === 'Escape') { e.stopPropagation(); onclose(); } };
    window.addEventListener('keydown', k, true);
    return () => window.removeEventListener('keydown', k, true);
  });
</script>

<div class="scrim" role="presentation" onmousedown={(e) => { if (e.target === e.currentTarget) onclose(); }}>
  <div class="modal glass-pop" role="dialog" aria-modal="true" aria-label={title} bind:this={el} style:width="{width}px">
    <h2>{title}</h2>
    <div class="body">{@render children()}</div>
    <div class="foot">{@render footer()}</div>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: var(--scrim); -webkit-backdrop-filter: blur(3px); backdrop-filter: blur(3px); display: flex; align-items: center; justify-content: center; z-index: 900; }
  .modal { max-width: calc(100vw - 32px); border-radius: var(--r-xl); overflow: hidden; }
  h2 { margin: 0; padding: 16px 20px 0; font-size: var(--fs-xl); font-weight: 600; }
  .body { padding: 12px 20px 18px; display: flex; flex-direction: column; gap: 12px; }
  .foot { display: flex; justify-content: flex-end; gap: 8px; padding: 12px 20px; background: var(--tint); border-top: 1px solid var(--border); }
</style>
