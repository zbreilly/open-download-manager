<script lang="ts">
  import { ICONS, type IconName } from './icons';
  import { themeState } from './themes.svelte';
  let { name, size = 16, stroke = 2, label = undefined }:
    { name: IconName; size?: number; stroke?: number; label?: string } = $props();
  // a colour profile can replace an icon with an imported image
  const custom = $derived(themeState.icons[name]);
</script>

{#if custom?.tint}
  <!-- drawn in the surrounding text colour, like the built-in icons -->
  <span class="mask" role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : 'true'}
    style:width="{size}px" style:height="{size}px" style:-webkit-mask-image="url({custom.url})" style:mask-image="url({custom.url})"></span>
{:else if custom}
  <img src={custom.url} width={size} height={size} alt={label ?? ''} aria-hidden={label ? undefined : 'true'} draggable="false" />
{:else}
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
       stroke-width={stroke} stroke-linecap="round" stroke-linejoin="round"
       role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : 'true'}
       style="flex:none">
    {#each ICONS[name] as s}
      {#if s.fill}<path d={s.d} fill="currentColor" stroke="none" />{:else}<path d={s.d} />{/if}
    {/each}
  </svg>
{/if}

<style>
  img, .mask { flex: none; display: inline-block; object-fit: contain; }
  .mask { background-color: currentColor; -webkit-mask: center / contain no-repeat; mask: center / contain no-repeat; }
</style>
