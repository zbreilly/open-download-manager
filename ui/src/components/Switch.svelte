<script lang="ts">
  let { checked = $bindable(false), label, onchange }: { checked?: boolean; label: string; onchange?: (v: boolean) => void } = $props();
</script>

<button type="button" role="switch" aria-checked={checked} aria-label={label} class="sw" class:on={checked}
  onclick={() => { checked = !checked; onchange?.(checked); }}>
  <span class="knob"></span>
</button>

<style>
  .sw { width: 40px; height: 22px; border-radius: 11px; border: 0; background: var(--switch-off); position: relative; padding: 0; flex: none; transition: background var(--motion); }
  .sw.on { background: var(--accent); }
  .knob { position: absolute; top: 3px; left: 3px; width: 16px; height: 16px; border-radius: 50%; background: var(--text-muted); transition: left var(--motion), background var(--motion); }
  .on .knob { left: 21px; background: var(--on-accent); }
</style>
