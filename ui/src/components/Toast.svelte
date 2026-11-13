<script lang="ts" module>
  export const toasts = $state({ list: [] as { id: number; text: string; kind: 'info' | 'error' }[] });
  let n = 0;
  export function toast(text: string, kind: 'info' | 'error' = 'info', ms = 4000) {
    const id = ++n;
    toasts.list.push({ id, text, kind });
    setTimeout(() => { toasts.list = toasts.list.filter((t) => t.id !== id); }, ms);
  }
</script>

<div class="toasts" aria-live="polite">
  {#each toasts.list as t (t.id)}
    <div class="toast glass-pop" class:error={t.kind === 'error'}>{t.text}</div>
  {/each}
</div>

<style>
  .toasts { position: fixed; bottom: 44px; right: 16px; display: flex; flex-direction: column; gap: 8px; z-index: 950; max-width: 420px; }
  .toast { padding: 10px 14px; border-radius: var(--r-md); border-left: 3px solid var(--accent); font-size: var(--fs-sm); user-select: text; -webkit-user-select: text; }
  .error { border-color: var(--danger); color: var(--danger); }
</style>
