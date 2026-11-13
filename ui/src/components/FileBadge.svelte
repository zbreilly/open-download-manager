<script lang="ts">
  import { extTone } from '../lib/format';
  let { ext, large = false }: { ext: string; large?: boolean } = $props();
</script>

<span class="badge c-{extTone(ext)}" class:large>{(ext || 'FILE').slice(0, 4)}</span>

<style>
  .badge {
    flex: none; width: 34px; height: 22px; border-radius: var(--r-sm); background: var(--c-soft);
    color: var(--c); box-shadow: inset 0 0 0 1px var(--c-soft); display: inline-flex; align-items: center; justify-content: center;
    font-family: var(--font-mono); font-size: 10px; font-weight: 600; letter-spacing: .02em;
  }
  .large { width: 40px; height: 40px; border-radius: 9px; font-size: 11px; }
</style>
