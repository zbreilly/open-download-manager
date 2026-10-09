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
  <div class="modal" role="dialog" aria-modal="true" aria-label={title} bind:this={el} style:width="{width}px">
    <h2>{title}</h2>
    <div class="body">{@render children()}</div>
    <div class="foot">{@render footer()}</div>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: rgba(0,0,0,.45); display: flex; align-items: center; justify-content: center; z-index: 900; }
  .modal { max-width: calc(100vw - 32px); background: var(--surface); border: 1px solid var(--border); border-radius: var(--r-xl); box-shadow: 0 20px 60px rgba(0,0,0,.5); overflow: hidden; }
  h2 { margin: 0; padding: 16px 20px 0; font-size: var(--fs-xl); font-weight: 600; }
  .body { padding: 12px 20px 18px; display: flex; flex-direction: column; gap: 12px; }
  .foot { display: flex; justify-content: flex-end; gap: 8px; padding: 12px 20px; background: var(--surface-2); border-top: 1px solid var(--border); }
</style>
