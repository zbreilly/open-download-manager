<script lang="ts">
  import { onMount } from 'svelte';
  import { initTheme } from './lib/theme';
  import Toast from './components/Toast.svelte';
  import Main from './windows/Main.svelte';
  import AddDownload from './windows/AddDownload.svelte';
  import Progress from './windows/Progress.svelte';
  import Scheduler from './windows/Scheduler.svelte';
  import Settings from './windows/Settings.svelte';
  import Grabber from './windows/Grabber.svelte';
  import About from './windows/About.svelte';
  import Theme from './windows/Theme.svelte';
  import { initThemeEngine } from './lib/themes.svelte';
  import { api, on, type Settings as AppSettings } from './lib/ipc';
  import TitleBar from './components/TitleBar.svelte';
  import { frame, wantsCustomFrame, VIEW_TITLES } from './lib/frame.svelte';

  const q = new URLSearchParams(location.search);
  const view = q.get('view') || 'main';
  const token = q.get('t') || '';
  const id = q.get('id') || '';
  let params: any = $state(null);
  let ready = $state(false);
  // the window was created with or without a native frame: draw ours to match
  frame.custom = q.get('frame') === 'custom';
  frame.title = VIEW_TITLES[view] ?? VIEW_TITLES.main;

  onMount(async () => {
    initTheme(q.get('theme') || 'dark');
    // the saved colour profile, if any, before the window shows its content
    await initThemeEngine();
    try { params = token ? await api.takeParams(token) : null; } catch { params = null; }
    try {
      const s = await api.getSettings();
      document.documentElement.dataset.density = s.density;
    } catch {}
    // the title bar setting changes open windows too
    on<AppSettings>('settings_changed', (s) => (frame.custom = wantsCustomFrame(s)));
    ready = true;
    // no browser context menu / reload shortcuts in production windows
    window.addEventListener('contextmenu', (e) => {
      const t = e.target as HTMLElement;
      if (!t.closest('input, textarea, .selectable')) e.preventDefault();
    });
    // a file dropped on a window must never replace the app's page (the
    // webview would open it); links and text are handled by the windows
    const refuseFiles = (e: DragEvent) => {
      if (!e.dataTransfer?.types.includes('Files')) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = 'none';
    };
    window.addEventListener('dragover', refuseFiles);
    window.addEventListener('drop', refuseFiles);
    window.addEventListener('keydown', (e) => {
      if ((e.key === 'r' && (e.metaKey || e.ctrlKey)) || e.key === 'F5') e.preventDefault();
    });
  });
</script>

<div class="window">
  <!-- the main window's own header is its title bar -->
  {#if frame.custom && view !== 'main'}<TitleBar maximizable={view !== 'about'} />{/if}
  <div class="view">
    {#if ready}
      {#if view === 'main'}<Main />
      {:else if view === 'add'}<AddDownload {params} />
      {:else if view === 'progress'}<Progress {id} />
      {:else if view === 'scheduler'}<Scheduler {params} />
      {:else if view === 'settings'}<Settings {params} />
      {:else if view === 'grabber'}<Grabber {params} />
      {:else if view === 'about'}<About />
      {:else if view === 'theme'}<Theme />
      {/if}
    {/if}
  </div>
</div>
<Toast />

<style>
  .window { height: 100%; display: flex; flex-direction: column; }
  .view { flex: 1; min-height: 0; position: relative; }
  /* views size themselves with height: 100% */
  .view > :global(*) { height: 100%; }
</style>
