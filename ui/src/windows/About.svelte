<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import Icon from '../lib/Icon.svelte';
  import { api } from '../lib/ipc';

  let info: { name: string; version: string; os: string; arch: string; dataDir: string } | null = $state(null);
  onMount(() => {
    const cleanups: (() => void)[] = [];
    (async () => {
    info = await api.appInfo();
    window.addEventListener('keydown', (e) => e.key === 'Escape' && getCurrentWindow().close());
    })();
    return () => cleanups.forEach((c) => c());
  });
</script>

<div class="about art-dialog">
  <span class="logo"><Icon name="odm-mark" size={40} stroke={2.4} /></span>
  <h1>Open Download Manager</h1>
  {#if info}
    <div class="mono muted">Version {info.version} · {info.os} {info.arch}</div>
    <div class="muted small">Segmented, resumable downloads with queues, scheduling and browser capture.</div>
    <button class="btn sm" onclick={() => api.openPath(info!.dataDir)}><Icon name="open-folder" size={14} />Open data folder</button>
  {/if}
  <div class="muted small">IBM Plex Sans and JetBrains Mono are used under the SIL Open Font License.</div>
</div>

<style>
  .about { height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 12px; padding: 24px; text-align: center; background: var(--glass); }
  .logo { width: 72px; height: 72px; border-radius: 18px; background: linear-gradient(140deg, var(--accent-hi), var(--violet)); color: #fff; box-shadow: inset 0 1px 0 rgba(255,255,255,.35), 0 8px 28px var(--accent-glow); display: flex; align-items: center; justify-content: center; }
  h1 { margin: 6px 0 0; font-size: 18px; font-weight: 600; }
  .small { font-size: var(--fs-sm); max-width: 320px; }
</style>
