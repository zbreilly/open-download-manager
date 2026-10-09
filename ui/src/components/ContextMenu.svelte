<script lang="ts" module>
  import type { IconName } from '../lib/icons';
  export type MenuItem =
    | { sep: true }
    | { label: string; icon?: IconName; action?: () => void; disabled?: boolean; danger?: boolean; sub?: MenuItem[]; checked?: boolean };
</script>

<script lang="ts">
  import Icon from '../lib/Icon.svelte';
  import { onMount } from 'svelte';
  let { x, y, items, onclose }: { x: number; y: number; items: MenuItem[]; onclose: () => void } = $props();
  let el = $state<HTMLDivElement | undefined>();
  let pos = $state({ left: 0, top: 0 });
  let openSub = $state(-1);

  onMount(() => {
    const r = el!.getBoundingClientRect();
    pos = {
      left: Math.min(x, window.innerWidth - r.width - 6),
      top: Math.min(y, window.innerHeight - r.height - 6),
    };
    el!.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
    const close = (e: Event) => { if (!el?.contains(e.target as Node)) onclose(); };
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onclose();
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const bs = [...el!.querySelectorAll<HTMLButtonElement>(':scope > button:not(:disabled)')];
        const i = bs.indexOf(document.activeElement as HTMLButtonElement);
        bs[(i + (e.key === 'ArrowDown' ? 1 : -1) + bs.length) % bs.length]?.focus();
      }
    };
    setTimeout(() => {
      window.addEventListener('mousedown', close);
      window.addEventListener('blur', onclose);
    });
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('mousedown', close);
      window.removeEventListener('blur', onclose);
      window.removeEventListener('keydown', key);
    };
  });

  function run(it: MenuItem) {
    if ('sep' in it || it.disabled || it.sub) return;
    onclose();
    it.action?.();
  }
</script>

<div class="menu" role="menu" bind:this={el} style:left="{pos.left}px" style:top="{pos.top}px">
  {#each items as it, i}
    {#if 'sep' in it}
      <div class="sep" role="separator"></div>
    {:else}
      <button type="button" role="menuitem" disabled={it.disabled} class:danger={it.danger}
        onmouseenter={() => (openSub = it.sub ? i : -1)} onclick={() => (it.sub ? (openSub = i) : run(it))}>
        <span class="ic">{#if it.checked}<Icon name="check" size={14} />{:else if it.icon}<Icon name={it.icon} size={16} />{/if}</span>
        <span class="lbl">{it.label}</span>
        {#if it.sub}<Icon name="chevron-right" size={14} />{/if}
      </button>
      {#if it.sub && openSub === i}
        <div class="sub">
          {#each it.sub as s}
            {#if 'sep' in s}<div class="sep"></div>{:else}
              <button type="button" role="menuitem" disabled={s.disabled} onclick={() => run(s)}>
                <span class="ic">{#if s.checked}<Icon name="check" size={14} />{:else if s.icon}<Icon name={s.icon} size={16} />{/if}</span>
                <span class="lbl">{s.label}</span>
              </button>
            {/if}
          {/each}
        </div>
      {/if}
    {/if}
  {/each}
</div>

<style>
  .menu, .sub {
    position: fixed; z-index: 1000; min-width: 210px; padding: 5px;
    background: var(--surface); border: 1px solid var(--border); border-radius: var(--r-md);
    box-shadow: 0 10px 30px rgba(0,0,0,.35); display: flex; flex-direction: column;
  }
  .sub { position: static; box-shadow: none; margin: 2px 0 2px 18px; min-width: 0; }
  button {
    display: flex; align-items: center; gap: 9px; height: 30px; padding: 0 10px;
    border: 0; border-radius: 6px; background: transparent; color: var(--text); text-align: left;
  }
  button:hover:not(:disabled), button:focus-visible { background: var(--accent-soft); outline: none; }
  .ic { width: 16px; display: flex; color: var(--text-muted); }
  .lbl { flex: 1; white-space: nowrap; }
  .danger { color: var(--danger); }
  .danger .ic { color: var(--danger); }
  .sep { height: 1px; background: var(--border); margin: 4px 6px; }
</style>
