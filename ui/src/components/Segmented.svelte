<script lang="ts" generics="T extends string | number">
  let { options, value = $bindable(), label, disabled = false, onchange }:
    { options: { value: T; label: string }[]; value: T; label: string; disabled?: boolean; onchange?: (v: T) => void } = $props();
</script>

<div role="radiogroup" aria-label={label} class="seg">
  {#each options as o}
    <button type="button" role="radio" aria-checked={o.value === value} class:on={o.value === value}
      disabled={disabled && o.value !== value}
      onclick={() => { value = o.value; onchange?.(o.value); }}>{o.label}</button>
  {/each}
</div>

<style>
  .seg { display: flex; gap: 4px; padding: 3px; border-radius: 9px; background: var(--surface-sunken); border: 1px solid var(--border); width: fit-content; }
  button { min-width: 44px; height: 30px; padding: 0 14px; border-radius: 6px; border: 0; background: transparent; color: var(--text-muted); }
  button.on { background: linear-gradient(180deg, var(--accent-hi), var(--accent-lo)); color: var(--on-accent); font-weight: 600; box-shadow: inset 0 1px 0 rgba(255,255,255,.28), 0 2px 8px var(--accent-glow); }
  button:hover:not(.on):not(:disabled) { color: var(--text); background: var(--hover); }
</style>
