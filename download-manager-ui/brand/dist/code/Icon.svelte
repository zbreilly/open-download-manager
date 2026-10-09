<script lang="ts">
  import { ICONS, type IconName } from './icons';
  let { name, size = 16, stroke = 2, label = undefined }:
    { name: IconName; size?: number; stroke?: number; label?: string } = $props();
</script>

<svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
     stroke-width={stroke} stroke-linecap="round" stroke-linejoin="round"
     role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : 'true'}>
  {#each ICONS[name] as s}
    {#if s.fill}<path d={s.d} fill="currentColor" stroke="none" />{:else}<path d={s.d} />{/if}
  {/each}
</svg>
