<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { readText } from '@tauri-apps/plugin-clipboard-manager';
  import { listen } from '@tauri-apps/api/event';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import Icon from '../lib/Icon.svelte';
  import FileBadge from '../components/FileBadge.svelte';
  import Segmented from '../components/Segmented.svelte';
  import { api, errText, type ProbeResult, type Queue, type Settings } from '../lib/ipc';
  import * as f from '../lib/format';

  let { params }: { params: any } = $props();
  const win = getCurrentWindow();

  let settings = $state<Settings | null>(null);
  let queues = $state<Queue[]>([]);
  let recents = $state<string[]>([]);

  let url = $state('');
  let batch = $state(false);
  let probing = $state(false);
  let probe = $state<ProbeResult | null>(null);
  let probeErr = $state('');
  let fileName = $state('');
  let nameTouched = false;
  let category = $state('general');
  let catTouched = false;
  let autoCat = $state('general');
  let alwaysCat = $state(false);
  const nameExt = $derived.by(() => { const i = fileName.lastIndexOf('.'); return i > 0 && fileName.length - i <= 12 ? fileName.slice(i + 1).toLowerCase() : ''; });
  let queueId = $state('');
  let saveDir = $state('');
  let dirTouched = false;
  let conns = $state(16);
  let remember = $state(true);
  let verify = $state(false);
  let checksum = $state('');
  let advanced = $state(false);
  let username = $state('');
  let password = $state('');
  let referrer = $state('');
  let cookies = $state('');
  let userAgent = $state('');
  let headers: { k: string; v: string }[] = $state([]);
  let exists = $state(false);
  let busy = $state(false);
  let submitErr = $state('');
  let batchUrls = $state<string[]>([]);
  let urlEl = $state<HTMLInputElement | HTMLTextAreaElement | undefined>();

  const isUrl = (s: string) => { try { const u = new URL(s.trim()); return u.protocol === 'http:' || u.protocol === 'https:'; } catch { return false; } };
  const urlValid = $derived(batch ? batchUrls.length > 0 : isUrl(url));
  const resumable = $derived(probe ? probe.resumable : true);
  const catName = $derived(settings?.categories.find((c) => c.id === category)?.name ?? 'this category');

  let timer: ReturnType<typeof setTimeout>;
  let probeSeq = 0;
  $effect(() => {
    const u = url.trim();
    const isBatch = batch;
    clearTimeout(timer);
    exists = false;
    if (isBatch) {
      timer = setTimeout(async () => {
        const lines = u.split(/\s+/).filter(Boolean);
        const out: string[] = [];
        for (const l of lines) {
          if (/\[[^\]]+-[^\]]+\]/.test(l)) out.push(...(await api.expandBatch(l)));
          else out.push(l);
        }
        batchUrls = out.filter(isUrl).slice(0, 10000);
      }, 300);
      return;
    }
    probe = null;
    probeErr = '';
    if (!isUrl(u)) { probing = false; return; }
    probing = true;
    const seq = ++probeSeq;
    timer = setTimeout(async () => {
      try {
        const p = await api.probe(u, {
          referrer: referrer || null, cookies: cookies || null,
          headers: headers.filter((h) => h.k).map((h) => [h.k, h.v]),
          username: username || null, password: password || null,
        });
        if (seq !== probeSeq) return;
        probe = p;
        if (!nameTouched) fileName = p.fileName;
        autoCat = p.category;
        if (!catTouched) await setCategory(p.category, false);
        if (!p.resumable) conns = 1;
        else if (conns === 1) conns = settings?.defaultConnections ?? 16;
      } catch (e) {
        if (seq !== probeSeq) return;
        probeErr = errText(e);
        if (!nameTouched) {
          try { fileName = decodeURIComponent(new URL(u).pathname.split('/').filter(Boolean).pop() || ''); } catch {}
        }
      } finally {
        if (seq === probeSeq) probing = false;
      }
    }, 300);
  });

  async function setCategory(c: string, touched = true) {
    category = c;
    if (touched) catTouched = true;
    if (!dirTouched) saveDir = await api.folderFor(c);
  }

  async function browse() {
    const p = await openDialog({ directory: true, defaultPath: saveDir || undefined });
    if (p) { saveDir = p as string; dirTouched = true; }
  }

  function req(start: boolean, link: string, onExists?: string) {
    return {
      url: link,
      fileName: batch ? null : fileName.trim() || null,
      saveDir: saveDir.trim() || null,
      category: batch ? null : category,
      queueId: queueId || (start ? null : 'main'),
      start,
      connections: conns,
      referrer: referrer || null,
      cookies: cookies || null,
      userAgent: userAgent || null,
      headers: headers.filter((h) => h.k.trim()).map((h) => [h.k.trim(), h.v] as [string, string]),
      username: username || null,
      password: password || null,
      checksum: !batch && verify && checksum.trim() ? { algo: '', expected: checksum.trim() } : null,
      onExists: onExists ?? 'rename',
      rememberFolder: !batch && remember && dirTouched,
      totalBytes: probe?.totalBytes ?? null,
      resumable: probe ? probe.resumable : null,
      mime: probe?.mime ?? null,
    };
  }

  async function submit(start: boolean, onExists?: string) {
    if (!urlValid || busy) return;
    submitErr = '';
    if (!batch && !onExists && fileName.trim() && saveDir.trim()) {
      if (await api.targetExists(saveDir.trim(), fileName.trim())) {
        exists = true;
        pendingStart = start;
        setTimeout(() => document.querySelector('.existbox')?.scrollIntoView({ behavior: 'smooth', block: 'nearest' }), 20);
        return;
      }
    }
    busy = true;
    try {
      if (!batch && alwaysCat && nameExt && category !== autoCat) await api.assignExtensions(category, [nameExt]);
      if (batch) await api.addMany(batchUrls.map((u) => req(start, u)));
      else await api.add(req(start, url.trim(), onExists));
      await win.close();
    } catch (e) {
      submitErr = errText(e);
      busy = false;
    }
  }
  let pendingStart = true;

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') { e.preventDefault(); win.close(); }
    if (e.key === 'Enter' && !(e.target as HTMLElement).closest('textarea') && !(e.target as HTMLElement).closest('button')) {
      e.preventDefault();
      submit(true);
    }
  }

  function chipLabel(p: string) {
    const parts = p.split(/[\\/]/).filter(Boolean);
    return parts.length > 2 ? '…/' + parts.slice(-2).join('/') : p;
  }

  function applyParams(p: any) {
    if (!p) return;
    if (p.batch) batch = true;
    if (p.url) url = p.url;
    if (p.fileName) { fileName = p.fileName; nameTouched = true; }
    if (p.referrer) referrer = p.referrer;
    if (p.cookies) cookies = p.cookies;
    if (p.userAgent) userAgent = p.userAgent;
  }

  onMount(() => {
    const cleanups: (() => void)[] = [];
    (async () => {
    const [s, q, r] = await Promise.all([api.getSettings(), api.listQueues(), api.recentFolders(4)]);
    settings = s;
    queues = q;
    recents = r;
    conns = s.defaultConnections;
    saveDir = await api.folderFor('general');
    applyParams(params);
    if (!url && !batch) {
      try {
        const t = (await readText())?.trim() ?? '';
        if (isUrl(t) && !/\s/.test(t)) url = t;
      } catch {}
    }
    setTimeout(() => { urlEl?.focus(); (urlEl as HTMLInputElement)?.select?.(); }, 30);
    const un = await listen('view_params', (e) => applyParams(e.payload));
    cleanups.push(un);
    })();
    return () => cleanups.forEach((c) => c());
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="dialog">
  <div class="dialog-head">
    <h1>{batch ? 'Add batch download' : 'Add download'}</h1>
    <span class="spacer"></span>
    <button class="btn link" onclick={() => { batch = !batch; probe = null; }}>
      <Icon name={batch ? 'download' : 'batch-download'} size={15} />{batch ? 'Single link' : 'Batch / multiple links'}
    </button>
  </div>

  <div class="dialog-body">
    <label class="field">
      <span>{batch ? 'Links (one per line; patterns like file[001-120].jpg expand)' : 'URL'}</span>
      {#if batch}
        <textarea bind:this={urlEl} class="input mono" rows="5" bind:value={url} spellcheck="false" placeholder="https://example.com/photos/img[001-050].jpg"></textarea>
      {:else}
        <input bind:this={urlEl} class="input mono" class:invalid={!!url && (!isUrl(url) || !!probeErr)} type="url" bind:value={url} spellcheck="false" placeholder="https://" />
      {/if}
      {#if url && !batch && !isUrl(url)}<span class="bad">Enter a full http:// or https:// address.</span>{/if}
    </label>

    {#if batch}
      <div class="info card">
        <FileBadge ext="LIST" large />
        <div class="infotext">
          <div class="mono">{batchUrls.length} {batchUrls.length === 1 ? 'file' : 'files'}</div>
          <div class="muted small ellipsis mono">{batchUrls[0] ?? 'Add links above'}{batchUrls.length > 1 ? ` … ${batchUrls[batchUrls.length - 1]}` : ''}</div>
        </div>
      </div>
    {:else if isUrl(url)}
      <div class="info card">
        <FileBadge ext={fileName.split('.').pop()?.toUpperCase().slice(0, 4) ?? ''} large />
        <div class="infotext">
          {#if probing}
            <div class="skeleton" style="width:60%"></div>
            <div class="muted small">Checking link…</div>
          {:else if probeErr}
            <div class="bad">{probeErr}</div>
            <div class="muted small">You can still add it; the download will retry when started.</div>
          {:else if probe}
            <div class="mono small">{f.bytes(probe.totalBytes)}{probe.mime ? ` · ${probe.mime}` : ''}</div>
            {#if probe.resumable}
              <div class="chips">
                <span><Icon name="check" size={13} />Resumable</span>
                <span><Icon name="check" size={13} />Byte ranges</span>
                <span>{probe.protocol}</span>
              </div>
            {:else}
              <div class="warnline"><Icon name="warning" size={14} />Server doesn't support resume. Download will use 1 connection.</div>
            {/if}
          {/if}
        </div>
      </div>
    {/if}

    {#if !batch}
      <label class="field">
        <span>File name</span>
        <input class="input" bind:value={fileName} oninput={() => (nameTouched = true)} spellcheck="false" />
      </label>
    {/if}

    <div class="grid2">
      {#if !batch}
        <label class="field">
          <span>Category</span>
          <select class="select" value={category} onchange={(e) => setCategory((e.target as HTMLSelectElement).value)}>
            {#each settings?.categories ?? [] as c}<option value={c.id}>{c.name}</option>{/each}
          </select>
        </label>
      {/if}
      <label class="field">
        <span>Queue</span>
        <select class="select" bind:value={queueId}>
          <option value="">None (start manually)</option>
          {#each queues as q}<option value={q.id}>{q.name}</option>{/each}
        </select>
      </label>
    </div>

    <div class="field">
      <label class="field">
        <span>Save to</span>
        <div class="row">
          <input class="input mono" style="flex:1" bind:value={saveDir} oninput={() => (dirTouched = true)} spellcheck="false" />
          <button class="btn" type="button" onclick={browse}>Browse</button>
        </div>
      </label>
      {#if recents.length}
        <div class="recents">
          <span class="muted small">Recent</span>
          {#each recents as r}
            <button type="button" class="chip mono" class:on={r === saveDir} title={r} onclick={() => { saveDir = r; dirTouched = true; }}>{chipLabel(r)}</button>
          {/each}
        </div>
      {/if}
    </div>

    <div class="field">
      <span>Connections</span>
      <Segmented label="Connections" bind:value={conns} disabled={!resumable}
        options={[1, 4, 8, 16, 32].map((n) => ({ value: n, label: String(n) }))} />
    </div>

    <div class="checks">
      {#if !batch}
        {#if nameExt && category !== autoCat}
          <label class="check"><input type="checkbox" bind:checked={alwaysCat} />Always put .{nameExt} files in {catName}</label>
        {/if}
        <label class="check"><input type="checkbox" bind:checked={remember} />Remember this folder for {catName}</label>
        <label class="check"><input type="checkbox" bind:checked={verify} />Verify checksum after download</label>
        {#if verify}
          <input class="input mono" bind:value={checksum} placeholder="SHA-256, SHA-1 or MD5 hex" spellcheck="false" />
        {/if}
      {/if}
    </div>

    <button type="button" class="btn link adv" onclick={() => (advanced = !advanced)} aria-expanded={advanced}>
      <Icon name={advanced ? 'chevron-down' : 'chevron-right'} size={14} />Advanced: authentication, referrer, cookies, headers
    </button>
    {#if advanced}
      <div class="advbox">
        <div class="grid2">
          <label class="field"><span>Username</span><input class="input" bind:value={username} autocomplete="off" /></label>
          <label class="field"><span>Password</span><input class="input" type="password" bind:value={password} autocomplete="off" /></label>
        </div>
        <label class="field"><span>Referrer</span><input class="input mono" bind:value={referrer} spellcheck="false" /></label>
        <label class="field"><span>Cookies</span><input class="input mono" bind:value={cookies} placeholder="name=value; other=value" spellcheck="false" /></label>
        <label class="field"><span>User agent (blank = default)</span><input class="input mono" bind:value={userAgent} spellcheck="false" /></label>
        <div class="field">
          <span>Headers</span>
          {#each headers as h, i}
            <div class="row">
              <input class="input mono" style="flex:1" placeholder="Header" bind:value={h.k} />
              <input class="input mono" style="flex:2" placeholder="Value" bind:value={h.v} />
              <button class="btn icon" aria-label="Remove header" onclick={() => headers.splice(i, 1)}><Icon name="minus" size={14} /></button>
            </div>
          {/each}
          <button class="btn sm" style="align-self:flex-start" onclick={() => headers.push({ k: '', v: '' })}><Icon name="plus" size={14} />Add header</button>
        </div>
      </div>
    {/if}

    {#if exists}
      <div class="card existbox">
        <div><Icon name="warning" size={14} /> <b>{fileName}</b> already exists in this folder.</div>
        <div class="row">
          <button class="btn sm" onclick={() => submit(pendingStart, 'overwrite')}>Overwrite</button>
          <button class="btn sm" onclick={() => submit(pendingStart, 'rename')}>Rename to “{fileName.replace(/(\.[^.]+)?$/, ' (1)$1')}”</button>
          <button class="btn sm" onclick={() => submit(pendingStart, 'resume')}>Resume existing</button>
        </div>
      </div>
    {/if}
    {#if submitErr}<div class="errbox">{submitErr}</div>{/if}
  </div>

  <div class="dialog-foot">
    <button class="btn" disabled={!urlValid || busy} onclick={() => submit(false)}><Icon name="add-to-queue" size={15} />Add to queue</button>
    <span class="spacer"></span>
    <button class="btn" onclick={() => win.close()}>Cancel</button>
    <button class="btn primary" disabled={!urlValid || busy} onclick={() => submit(true)}>
      {batch ? `Start ${batchUrls.length || ''} downloads` : 'Start download'}
    </button>
  </div>
</div>

<style>
  .bad { color: var(--danger); font-size: var(--fs-sm); }
  .info { display: flex; align-items: center; gap: 12px; padding: 12px; }
  .infotext { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 5px; }
  .chips { display: flex; gap: 14px; font-size: var(--fs-sm); color: var(--text-muted); flex-wrap: wrap; }
  .chips span { display: flex; align-items: center; gap: 5px; }
  .chips :global(svg) { color: var(--success); }
  .small { font-size: var(--fs-sm); }
  .grid2 { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .recents { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
  .chip { height: 28px; padding: 0 10px; border-radius: 7px; font-size: 11px; border: 1px solid var(--border); background: transparent; color: var(--text-muted); max-width: 220px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chip.on { border-color: var(--accent); background: var(--accent-soft); color: var(--text); }
  .checks { display: flex; flex-direction: column; gap: 8px; }
  .adv { align-self: flex-start; display: flex; gap: 6px; }
  .advbox { display: flex; flex-direction: column; gap: 12px; padding: 12px; border: 1px solid var(--border); border-radius: var(--r-lg); }
  .existbox { padding: 12px; display: flex; flex-direction: column; gap: 10px; border-color: var(--warning); }
  .existbox :global(svg) { color: var(--warning); vertical-align: -2px; }
</style>
