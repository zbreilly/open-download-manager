<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { listen } from '@tauri-apps/api/event';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import Icon from '../lib/Icon.svelte';
  import FileBadge from '../components/FileBadge.svelte';
  import { api, errText, type GrabbedLink, type Queue, type Settings } from '../lib/ipc';

  let { params }: { params: any } = $props();
  const win = getCurrentWindow();

  let settings = $state<Settings | null>(null);
  let queues = $state<Queue[]>([]);
  let url = $state('');
  let depth = $state(0);
  let preset = $state('all');
  let custom = $state('');
  let sameHost = $state(true);
  let busy = $state(false);
  let err = $state('');
  let links = $state<(GrabbedLink & { on: boolean })[]>([]);
  let filterText = $state('');
  let saveDir = $state('');
  let queueId = $state('');
  let fromList = $state(false);
  let referrer: string | null = null;
  let cookies: string | null = null;
  let userAgent: string | null = null;

  const PRESETS: Record<string, string> = {
    all: '',
    images: 'jpg jpeg png gif webp bmp svg tif tiff avif heic',
    video: 'mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts',
    audio: 'mp3 wav flac aac m4a ogg opus wma',
    documents: 'pdf doc docx xls xlsx ppt pptx odt ods txt epub csv',
    archives: 'zip rar 7z tar gz tgz bz2 xz zst',
    programs: 'exe msi dmg pkg deb rpm appimage apk iso',
  };
  const exts = $derived((preset === 'custom' ? custom : PRESETS[preset]).split(/[\s,;]+/).filter(Boolean));
  const visible = $derived(links.filter((l) => !filterText || l.url.toLowerCase().includes(filterText.toLowerCase())));
  const chosen = $derived(links.filter((l) => l.on));

  async function run() {
    err = '';
    busy = true;
    fromList = false;
    try {
      const r = await api.grab(url.trim(), depth, exts, sameHost);
      links = r.map((l) => ({ ...l, on: true }));
      referrer = url.trim();
      if (!r.length) err = 'No matching files were found on that page.';
    } catch (e) {
      err = errText(e);
    } finally {
      busy = false;
    }
  }

  function nameOf(u: string) {
    try { return decodeURIComponent(new URL(u).pathname.split('/').filter(Boolean).pop() || 'download'); } catch { return u; }
  }
  function extOf(n: string) {
    const i = n.lastIndexOf('.');
    return i > 0 ? n.slice(i + 1).toUpperCase().slice(0, 4) : 'FILE';
  }

  async function add(start: boolean) {
    const reqs = chosen.map((l) => ({
      url: l.url, start, saveDir: saveDir.trim() || null, queueId: queueId || (start ? null : 'main'),
      referrer, cookies, userAgent, connections: settings?.defaultConnections,
    }));
    if (!reqs.length) return;
    busy = true;
    try {
      await api.addMany(reqs);
      await win.close();
    } catch (e) {
      err = errText(e);
      busy = false;
    }
  }

  function applyParams(p: any) {
    if (!p) return;
    if (p.url) url = p.url;
    if (p.links?.length) {
      fromList = true;
      links = p.links.map((l: any) => {
        const n = l.fileName || nameOf(l.url);
        return { url: l.url, fileName: n, ext: extOf(n), category: '', source: 'link', on: true };
      });
    }
    referrer = p.referrer ?? referrer;
    cookies = p.cookies ?? cookies;
    userAgent = p.userAgent ?? userAgent;
  }

  onMount(() => {
    const cleanups: (() => void)[] = [];
    (async () => {
    [settings, queues] = await Promise.all([api.getSettings(), api.listQueues()]);
    saveDir = settings.defaultFolder;
    applyParams(params);
    const un = await listen('view_params', (e) => applyParams(e.payload));
    const k = (e: KeyboardEvent) => { if (e.key === 'Escape') win.close(); };
    window.addEventListener('keydown', k);
    cleanups.push(() => { un(); window.removeEventListener('keydown', k); });
    })();
    return () => cleanups.forEach((c) => c());
  });
</script>

<div class="dialog">
  <div class="dialog-head">
    <Icon name="site-grabber" size={20} />
    <h1>{fromList ? 'Download links' : 'Site grabber'}</h1>
  </div>
  <div class="dialog-body">
    {#if !fromList}
      <div class="row">
        <label class="field" style="flex:1">
          <span>Page address</span>
          <input class="input mono" bind:value={url} placeholder="https://example.com/gallery/" spellcheck="false" onkeydown={(e) => e.key === 'Enter' && run()} />
        </label>
        <label class="field">
          <span>Depth</span>
          <select class="select" bind:value={depth}>
            <option value={0}>This page</option>
            <option value={1}>+1 level</option>
            <option value={2}>+2 levels</option>
          </select>
        </label>
        <label class="field">
          <span>File types</span>
          <select class="select" bind:value={preset}>
            <option value="all">All files</option>
            <option value="images">Images</option>
            <option value="video">Video</option>
            <option value="audio">Audio</option>
            <option value="documents">Documents</option>
            <option value="archives">Archives</option>
            <option value="programs">Programs</option>
            <option value="custom">Custom…</option>
          </select>
        </label>
        <button class="btn primary" style="align-self:flex-end" disabled={busy || !url.trim()} onclick={run}>{busy ? 'Scanning…' : 'Find files'}</button>
      </div>
      {#if preset === 'custom'}<input class="input mono" bind:value={custom} placeholder="jpg png zip" />{/if}
      {#if depth > 0}<label class="check"><input type="checkbox" bind:checked={sameHost} />Stay on the same site</label>{/if}
    {/if}

    {#if err}<div class="errbox">{err}</div>{/if}

    <div class="listhead">
      <span class="muted small">{chosen.length} of {links.length} selected</span>
      <button class="btn link small" onclick={() => links.forEach((l) => (l.on = true))}>Select all</button>
      <button class="btn link small" onclick={() => links.forEach((l) => (l.on = false))}>None</button>
      <span class="spacer"></span>
      <input class="input filt" bind:value={filterText} placeholder="Filter" />
    </div>
    <div class="list card">
      {#each visible as l (l.url)}
        <label class="item">
          <input type="checkbox" bind:checked={l.on} />
          <FileBadge ext={l.ext} />
          <span class="ellipsis nm">{l.fileName}</span>
          <span class="ellipsis mono muted small u" title={l.url}>{l.url}</span>
        </label>
      {:else}
        <div class="muted empty">{busy ? 'Scanning…' : 'Links will appear here.'}</div>
      {/each}
    </div>

    <div class="row">
      <label class="field" style="flex:2">
        <span>Save to</span>
        <div class="row">
          <input class="input mono" style="flex:1" bind:value={saveDir} />
          <button class="btn" onclick={async () => { const p = await openDialog({ directory: true, defaultPath: saveDir }); if (p) saveDir = p as string; }}>Browse</button>
        </div>
      </label>
      <label class="field" style="flex:1">
        <span>Queue</span>
        <select class="select" bind:value={queueId}>
          <option value="">None (start now)</option>
          {#each queues as q}<option value={q.id}>{q.name}</option>{/each}
        </select>
      </label>
    </div>
  </div>
  <div class="dialog-foot">
    <button class="btn" disabled={!chosen.length || busy} onclick={() => add(false)}><Icon name="add-to-queue" size={15} />Add to queue</button>
    <span class="spacer"></span>
    <button class="btn" onclick={() => win.close()}>Cancel</button>
    <button class="btn primary" disabled={!chosen.length || busy} onclick={() => add(true)}>Download {chosen.length || ''}</button>
  </div>
</div>

<style>
  .row { display: flex; gap: 10px; align-items: flex-start; flex-wrap: wrap; }
  .small { font-size: var(--fs-sm); }
  .listhead { display: flex; align-items: center; gap: 12px; }
  .filt { height: 30px; width: 200px; }
  .list { flex: 1; min-height: 200px; overflow: auto; background: var(--surface); }
  .item { display: grid; grid-template-columns: 16px 34px minmax(120px, 1fr) minmax(0, 1.4fr); gap: 10px; align-items: center; padding: 6px 12px; border-bottom: 1px solid var(--border-subtle); }
  .item:hover { background: var(--surface-2); }
  .nm { font-weight: 500; }
  .empty { padding: 30px; text-align: center; }
</style>
