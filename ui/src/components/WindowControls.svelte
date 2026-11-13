<script lang="ts">
  // Minimise / maximise / close for windows that draw their own title bar.
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  let { maximizable = true }: { maximizable?: boolean } = $props();
  const win = getCurrentWindow();
  let maxed = $state(false);

  onMount(() => {
    const sync = () => win.isMaximized().then((m) => (maxed = m)).catch(() => {});
    sync();
    // maximising by double-click, snapping or the keyboard all resize the window
    window.addEventListener('resize', sync);
    return () => window.removeEventListener('resize', sync);
  });
</script>

<div class="wc">
  <button aria-label="Minimise" title="Minimise" onclick={() => win.minimize()}>
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" /></svg>
  </button>
  {#if maximizable}
    <button aria-label={maxed ? 'Restore' : 'Maximise'} title={maxed ? 'Restore' : 'Maximise'} onclick={() => win.toggleMaximize()}>
      {#if maxed}
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M2.5 2.5V.5h7v7h-2M.5 2.5h7v7h-7z" /></svg>
      {:else}
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M.5.5h9v9h-9z" /></svg>
      {/if}
    </button>
  {/if}
  <button class="x" aria-label="Close" title="Close" onclick={() => win.close()}>
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" /></svg>
  </button>
</div>

<style>
  .wc { display: flex; align-self: stretch; flex: none; }
  button {
    width: 44px; min-height: 32px; height: 100%; padding: 0; border: 0; background: transparent; color: var(--text-muted);
    display: flex; align-items: center; justify-content: center; transition: background var(--motion), color var(--motion);
  }
  svg { fill: none; stroke: currentColor; stroke-width: 1; }
  button:hover { background: var(--hover); color: var(--text); }
  button.x:hover { background: #E81123; color: #fff; }
  button:focus-visible { outline-offset: -2px; }
</style>
