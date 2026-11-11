<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import Icon from '../lib/Icon.svelte';
  import { ICONS, type IconName } from '../lib/icons';
  import Switch from '../components/Switch.svelte';
  import Segmented from '../components/Segmented.svelte';
  import { api, errText, on, type Category, type Settings } from '../lib/ipc';
  import { MBPS, parseLimitMB, LIMIT_ERROR } from '../lib/format';
  import { toast } from '../components/Toast.svelte';
  import { isMac } from '../lib/theme';

  let { params }: { params: any } = $props();
  const win = getCurrentWindow();

  const PAGES: { id: string; label: string; icon: IconName }[] = [
    { id: 'general', label: 'General', icon: 'options' },
    { id: 'connection', label: 'Connection & speed', icon: 'segments' },
    { id: 'browser', label: 'Browser integration', icon: 'browser-integration' },
    { id: 'categories', label: 'File types & categories', icon: 'file-types' },
    { id: 'locations', label: 'Save locations', icon: 'open-folder' },
    { id: 'proxy', label: 'Proxy', icon: 'proxy' },
    { id: 'notifications', label: 'Notifications', icon: 'notifications' },
    { id: 'appearance', label: 'Appearance', icon: 'theme-dark' },
    { id: 'advanced', label: 'Advanced', icon: 'properties' },
  ];
  let page = $state('connection');
  let s = $state<Settings | null>(null);
  /** Latest settings known from the engine. A field whose value still equals
   *  this is untouched, so changes made elsewhere (tray, toolbar, another
   *  window) flow into it; fields the user edited are left alone. */
  let base: Settings | null = null;
  let busy = $state(false);
  let ext = $state({ connected: false, browsers: [] as string[], port: 8901, bridgeError: null as string | null });

  // MB/s editing helpers
  let globalMB = $state('25');
  let perMB = $state('5');
  let captureExts = $state('');
  let pausedNow = $state(false);

  const fmtMB = (bps: number) => String(Math.round((bps / MBPS) * 10) / 10);
  const extsText = (x: Settings) => x.captureExtensions.join(' ');
  const pausedOf = (x: Settings) => !!x.capturePausedUntil && x.capturePausedUntil > Date.now();
  const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

  /** Show `x` in the form (text helpers included). */
  function fill(x: Settings) {
    s = structuredClone(x);
    globalMB = fmtMB(x.globalLimitBps);
    perMB = fmtMB(x.perDownloadLimitBps);
    captureExts = extsText(x);
    pausedNow = pausedOf(x);
  }

  function load(x: Settings) {
    base = structuredClone(x);
    fill(x);
  }

  /** settings_changed: take the new values for every field the user hasn't edited. */
  function merge(incoming: Settings) {
    if (!s || !base) return load(incoming);
    const cur = $state.snapshot(s) as Settings;
    const b = base;
    for (const k of Object.keys(incoming) as (keyof Settings)[]) {
      if (same(cur[k], b[k])) (s as unknown as Record<string, unknown>)[k] = structuredClone(incoming[k]);
    }
    if (globalMB === fmtMB(b.globalLimitBps)) globalMB = fmtMB(incoming.globalLimitBps);
    if (perMB === fmtMB(b.perDownloadLimitBps)) perMB = fmtMB(incoming.perDownloadLimitBps);
    if (captureExts === extsText(b)) captureExts = extsText(incoming);
    if (pausedNow === pausedOf(b)) pausedNow = pausedOf(incoming);
    base = structuredClone(incoming);
  }

  function collect(): Settings | null {
    const x = $state.snapshot(s!) as Settings;
    // a text field still showing the stored value keeps the exact stored value
    const limit = (text: string, cur: number) => (text === fmtMB(cur) ? cur : parseLimitMB(text));
    const g = limit(globalMB, x.globalLimitBps), p = limit(perMB, x.perDownloadLimitBps);
    if (g == null || p == null) {
      toast(`${g == null ? 'Global' : 'Per-download'} limit: ${LIMIT_ERROR}`, 'error');
      return null;
    }
    x.globalLimitBps = g;
    x.perDownloadLimitBps = p;
    x.captureExtensions = captureExts.split(/[\s,;]+/).map((e) => e.replace(/^\./, '').toLowerCase()).filter(Boolean);
    x.defaultConnections = Math.max(1, Math.min(32, Number(x.defaultConnections) || 16));
    x.maxSimultaneous = Math.max(1, Math.min(64, Number(x.maxSimultaneous) || 4));
    x.proxyPort = Number(x.proxyPort) || 0;
    x.maxRetries = Math.max(0, Number(x.maxRetries) || 0);
    x.retryDelaySecs = Math.max(0, Number(x.retryDelaySecs) || 0);
    x.timeoutSecs = Math.max(5, Number(x.timeoutSecs) || 30);
    x.minSegmentKb = Math.max(64, Number(x.minSegmentKb) || 512);
    x.extensionPort = Number(x.extensionPort) || 8901;
    x.captureMinSizeKb = Math.max(0, Number(x.captureMinSizeKb) || 0);
    x.capturePausedUntil = pausedNow ? (x.capturePausedUntil && x.capturePausedUntil > Date.now() ? x.capturePausedUntil : Date.now() + 10 * 60_000) : null;
    return x;
  }

  async function save(close = true) {
    if (!s) return;
    const x = collect();
    if (!x) return;
    busy = true;
    try {
      await api.saveSettings(x);
      if (close) await win.close();
      else toast('Settings saved');
    } catch (e) {
      toast(errText(e), 'error');
    } finally {
      busy = false;
    }
  }
  async function restore() {
    const d = await api.defaultSettings();
    fill(d); // not the engine's state: every changed field stays edited
    toast('Defaults restored — press Save to keep them');
  }
  async function pickFolder(cur: string | null, set: (v: string) => void) {
    const p = await openDialog({ directory: true, defaultPath: cur || undefined });
    if (p) set(p as string);
  }
  function previewTheme(t: string) {
    const resolved = t === 'system' ? (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark') : t;
    document.documentElement.dataset.theme = resolved;
  }
  const CAT_ICONS = [
    { id: 'cat-video', label: 'Video' }, { id: 'cat-music', label: 'Music' }, { id: 'cat-documents', label: 'Documents' },
    { id: 'cat-compressed', label: 'Archive' }, { id: 'cat-programs', label: 'Programs' }, { id: 'cat-images', label: 'Images' },
    { id: 'cat-general', label: 'General' }, { id: 'file-disc', label: 'Disc' }, { id: 'cat-grabber', label: 'Web' },
  ];
  let moved: Record<string, string> = $state({});
  function sep() { return (s?.defaultFolder ?? '').includes('\\') ? '\\' : '/'; }
  function defaultFolderFor(c: Category) {
    if (!s) return '';
    if (c.id === 'general') return s.defaultFolder;
    const base = s.defaultFolder.replace(/[\\/]+$/, '');
    return `${base}${sep()}${c.name.trim().replace(/[\\/:*?"<>|]/g, '_') || 'Category'}`;
  }
  function addExt(c: Category, el: HTMLInputElement) {
    const exts = el.value.split(/[\s,;]+/).map((x) => x.replace(/^\.+/, '').toLowerCase()).filter(Boolean);
    el.value = '';
    const notes: string[] = [];
    for (const ext of exts) {
      for (const other of s!.categories) {
        if (other !== c && other.extensions.includes(ext)) {
          other.extensions = other.extensions.filter((x) => x !== ext);
          notes.push(`.${ext} moved from ${other.name}`);
        }
      }
      if (!c.extensions.includes(ext)) c.extensions = [...c.extensions, ext];
    }
    moved[c.id] = notes.join(' · ');
  }
  function addCategory() {
    s!.categories.splice(s!.categories.length - 1, 0, { id: 'c' + Math.random().toString(36).slice(2, 8), name: 'New category', icon: 'cat-general', extensions: [], folder: null });
  }

  onMount(() => {
    const cleanups: (() => void)[] = [];
    (async () => {
    load(await api.getSettings());
    if (params?.page) page = params.page;
    const st = await api.extensionStatus();
    ext = { ...st, bridgeError: st.bridgeError ?? null };
    const un = await on<{ connected: boolean; browsers: string[] }>('extension_status', (e) => (ext = { ...ext, ...e }));
    const unBridge = await on<{ port: number; error: string } | null>('bridge_error', (e) => (ext = { ...ext, bridgeError: e?.error ?? null }));
    const unSettings = await on<Settings>('settings_changed', merge);
    const k = (e: KeyboardEvent) => {
      if (e.key === 'Escape') win.close();
      if ((isMac ? e.metaKey : e.ctrlKey) && e.key === 's') { e.preventDefault(); save(false); }
    };
    window.addEventListener('keydown', k);
    cleanups.push(() => { un(); unBridge(); unSettings(); window.removeEventListener('keydown', k); });
    })();
    return () => cleanups.forEach((c) => c());
  });
  const BROWSERS = ['Chrome', 'Edge', 'Brave', 'Firefox', 'Opera', 'Vivaldi'];
</script>

<div class="wrap">
  <nav aria-label="Settings sections">
    <h1>Settings</h1>
    {#each PAGES as p}
      <button class="nav" class:on={page === p.id} onclick={() => (page = p.id)}><Icon name={p.icon} size={16} />{p.label}</button>
    {/each}
  </nav>

  <div class="content">
    <div class="scroll">
      {#if s}
        {#if page === 'general'}
          <section>
            <h2>General</h2>
            <div class="group card">
              <div class="rowc"><div class="t"><div class="tt">Launch at login</div><div class="d">Start minimized in the {isMac ? 'menu bar' : 'system tray'} when you sign in.</div></div><Switch label="Launch at login" bind:checked={s.launchAtLogin} /></div>
              <div class="rowc"><div class="t"><div class="tt">Keep running when the window is closed</div><div class="d">Closing the main window hides it; downloads continue. Quit from the {isMac ? 'menu bar icon' : 'tray icon'}.</div></div><Switch label="Minimize to tray" bind:checked={s.minimizeToTray} /></div>
              <div class="rowc"><div class="t"><div class="tt">Watch the clipboard for download links</div><div class="d">Copying a link to a file type listed under Browser integration opens the Add dialog.</div></div><Switch label="Clipboard monitoring" bind:checked={s.clipboardMonitor} /></div>
              <div class="rowc"><div class="t"><div class="tt">Show progress window when a download starts</div><div class="d">Off: progress stays in the main list.</div></div><Switch label="Show progress window on start" bind:checked={s.showProgressOnStart} /></div>
              <div class="rowc"><div class="t"><div class="tt">Show the download window when a download completes</div><div class="d">In addition to the notification.</div></div><Switch label="Show completion window" bind:checked={s.showCompleteDialog} /></div>
              <div class="rowc"><div class="t"><div class="tt">Confirm before deleting</div><div class="d">Ask before removing downloads from the list.</div></div><Switch label="Confirm before delete" bind:checked={s.confirmDelete} /></div>
            </div>
          </section>
        {:else if page === 'connection'}
          <section>
            <h2>Connection &amp; speed</h2>
            <div class="rowc card">
              <div class="t"><div class="tt">Default connections per download</div><div class="d">Files are split dynamically; idle connections take over the largest remaining segment.</div></div>
              <input type="range" min="1" max="32" bind:value={s.defaultConnections} aria-label="Default connections per download" style="width:180px" />
              <span class="mono num">{s.defaultConnections}</span>
            </div>
            <div class="rowc card">
              <div class="t"><div class="tt">Simultaneous downloads</div><div class="d">Outside of queues. Queues set their own limit.</div></div>
              <input class="input mono small-in" bind:value={s.maxSimultaneous} inputmode="numeric" aria-label="Simultaneous downloads" />
            </div>
            <div class="group card">
              <div class="rowc">
                <div class="t"><div class="tt">Global speed limit</div><div class="d">Caps the combined speed of all downloads.</div></div>
                <input class="input mono lim" class:dim={!s.globalLimitEnabled} bind:value={globalMB} inputmode="decimal" aria-label="Global speed limit" />
                <span class="mono muted unit">MB/s</span>
                <Switch label="Global speed limit on" bind:checked={s.globalLimitEnabled} />
              </div>
              <div class="rowc">
                <div class="t"><div class="tt">Per-download limit</div><div class="d">Applies to each download individually.</div></div>
                <input class="input mono lim" class:dim={!s.perDownloadLimitEnabled} bind:value={perMB} inputmode="decimal" aria-label="Per-download limit" />
                <span class="mono muted unit">MB/s</span>
                <Switch label="Per-download limit on" bind:checked={s.perDownloadLimitEnabled} />
              </div>
              <label class="rowc check"><input type="checkbox" bind:checked={s.limitOnStartup} />Turn on the limit when the app starts</label>
            </div>
          </section>
        {:else if page === 'browser'}
          <section>
            <h2>Browser integration</h2>
            <div class="group card">
              <div class="rowc">
                <div class="t"><div class="tt">Extension status</div><div class="d">The extension talks to the app on <span class="mono">127.0.0.1:{s.extensionPort}</span>. Only an extension paired with this app is accepted.</div>{#if ext.bridgeError}<div class="d bridge-err">{ext.bridgeError}. Another app may be using this port; pick a different one under Advanced.</div>{/if}</div>
                <span class="status"><span class="dot" class:ok={ext.connected && !ext.bridgeError} class:bad={!!ext.bridgeError}></span>{ext.bridgeError ? 'Not listening' : ext.connected ? 'Connected' : 'Not detected'}</span>
              </div>
              <div class="rowc">
                <div class="t"><div class="tt">Pairing code</div><div class="d">Paste it into the extension’s popup, under Connection. Only a paired extension can talk to the app, and other programs can’t read what it sends.</div></div>
                <button class="btn sm" onclick={async () => { await writeText((await api.extensionStatus()).pairingCode); toast('Pairing code copied'); }}>Copy</button>
                <button class="btn sm" onclick={async () => { await writeText(await api.resetPairing()); toast('New pairing code copied. Pair each browser again.'); }}>New code</button>
              </div>
              <div class="rowc">
                <div class="t"><div class="tt">Install the extension</div><div class="d">Chrome, Edge, Brave, Opera, Vivaldi: open the extensions page, turn on Developer mode, choose “Load unpacked” and pick the <span class="mono">chromium</span> folder. Firefox: about:debugging → This Firefox → Load Temporary Add-on → <span class="mono">firefox/manifest.json</span>.</div></div>
                <button class="btn sm" onclick={async () => { const d = await api.extensionDir(); if (d) api.openPath(d); else toast('Extension files not found in this build', 'error'); }}><Icon name="open-folder" size={14} />Show extension folder</button>
              </div>
              {#each BROWSERS as b}
                <div class="rowc">
                  <div class="t"><div class="tt">{b}</div></div>
                  {#if ext.browsers.includes(b)}
                    <span class="status"><span class="dot ok"></span>Connected</span>
                  {:else}
                    <span class="muted small">Not detected</span>
                  {/if}
                </div>
              {/each}
            </div>
            <div class="group card">
              <div class="rowc">
                <div class="t"><div class="tt">Take over browser downloads</div><div class="d">Matching downloads are handed to Open Download Manager instead of the browser.</div></div>
                <Switch label="Capture downloads" bind:checked={s.captureEnabled} />
              </div>
              <div class="rowc">
                <div class="t"><div class="tt">Pause capture for 10 minutes</div><div class="d">Let the browser handle downloads for a while. Turns itself back on.</div></div>
                <Switch label="Pause capture" bind:checked={pausedNow} />
              </div>
              <div class="rowc">
                <div class="t"><div class="tt">Bypass key</div><div class="d">Hold this key while clicking a link to let the browser download it.</div></div>
                <select class="select" bind:value={s.bypassKey} style="width:140px">
                  <option value="alt">{isMac ? 'Option' : 'Alt'}</option>
                  <option value="shift">Shift</option>
                  <option value="ctrl">{isMac ? 'Control' : 'Ctrl'}</option>
                  <option value="none">None</option>
                </select>
              </div>
              <div class="rowc">
                <div class="t"><div class="tt">Skip files smaller than</div><div class="d">0 captures everything that matches.</div></div>
                <input class="input mono small-in" bind:value={s.captureMinSizeKb} inputmode="numeric" /><span class="mono muted unit">KB</span>
              </div>
            </div>
            <label class="field">
              <span>File types to capture (space-separated extensions)</span>
              <textarea class="input mono" rows="4" bind:value={captureExts} spellcheck="false"></textarea>
            </label>
          </section>
        {:else if page === 'categories'}
          <section>
            <h2>File types &amp; categories</h2>
            <p class="muted small" style="margin:0">Each file type belongs to one category. New downloads are sorted by type and saved to the category's folder, which is a subfolder of your download folder unless you choose another. Anything not listed goes to General.</p>
            {#each s.categories as c, i (c.id)}
              <div class="catcard card">
                <div class="cathead">
                  <select class="select iconsel" bind:value={c.icon} aria-label="Icon for {c.name}">
                    {#each CAT_ICONS as ic}<option value={ic.id}>{ic.label}</option>{/each}
                  </select>
                  <Icon name={(c.icon in ICONS ? c.icon : 'cat-general') as IconName} size={18} />
                  <input class="input catname" bind:value={c.name} aria-label="Category name" disabled={c.id === 'general'} />
                  <div class="fold">
                    <input class="input mono" value={c.folder ?? ''} placeholder={defaultFolderFor(c)}
                      onchange={(e) => (c.folder = (e.target as HTMLInputElement).value.trim() || null)} aria-label="Folder for {c.name}" />
                    <button class="btn icon" aria-label="Choose folder" title="Choose folder" onclick={() => pickFolder(c.folder ?? defaultFolderFor(c), (v) => (c.folder = v))}><Icon name="open-folder" size={15} /></button>
                    {#if c.folder}<button class="btn icon" aria-label="Use default folder" title="Use the default subfolder" onclick={() => (c.folder = null)}><Icon name="refresh-link" size={15} /></button>{/if}
                  </div>
                  <button class="btn icon" aria-label="Delete category" title="Delete category (its types go to General)" disabled={c.id === 'general'} onclick={() => s!.categories.splice(i, 1)}><Icon name="delete" size={15} /></button>
                </div>
                {#if c.id === 'general'}
                  <div class="muted small">Everything that no other category claims.</div>
                {:else}
                  <div class="chips">
                    {#each c.extensions as ext (ext)}
                      <span class="chip mono">.{ext}<button aria-label="Remove .{ext}" onclick={() => (c.extensions = c.extensions.filter((x) => x !== ext))}><Icon name="close" size={11} stroke={2.4} /></button></span>
                    {/each}
                    <input class="chipin mono" placeholder="add type…" aria-label="Add file type to {c.name}"
                      onkeydown={(e) => { if (['Enter', ' ', ',', 'Tab'].includes(e.key) && (e.target as HTMLInputElement).value.trim()) { e.preventDefault(); addExt(c, (e.target as HTMLInputElement)); } }}
                      onblur={(e) => (e.target as HTMLInputElement).value.trim() && addExt(c, e.target as HTMLInputElement)} />
                  </div>
                  {#if moved[c.id]}<div class="muted small">{moved[c.id]}</div>{/if}
                {/if}
              </div>
            {/each}
            <button class="btn sm" style="align-self:flex-start" onclick={addCategory}><Icon name="plus" size={14} />Add category</button>
          </section>
        {:else if page === 'locations'}
          <section>
            <h2>Save locations</h2>
            <div class="rowc card">
              <div class="t"><div class="tt">Default download folder</div><div class="d">Used when a category has no folder of its own.</div></div>
              <input class="input mono path" bind:value={s.defaultFolder} />
              <button class="btn" onclick={() => pickFolder(s!.defaultFolder, (v) => (s!.defaultFolder = v))}>Browse</button>
            </div>
            <div class="group card">
              {#each s.categories as c (c.id)}
                <div class="rowc">
                  <div class="t"><div class="tt">{c.name}</div></div>
                  <input class="input mono path" value={c.folder ?? ''} placeholder={defaultFolderFor(c)} onchange={(e) => (c.folder = (e.target as HTMLInputElement).value || null)} />
                  <button class="btn" onclick={() => pickFolder(c.folder ?? defaultFolderFor(c), (v) => (c.folder = v))}>Browse</button>
                </div>
              {/each}
            </div>
            <p class="muted small">Unfinished downloads are kept next to the final file as <span class="mono">name.odmpart</span> and renamed when complete.</p>
          </section>
        {:else if page === 'proxy'}
          <section>
            <h2>Proxy</h2>
            <div class="rowc card">
              <div class="t"><div class="tt">Connection</div><div class="d">System uses your OS proxy settings and HTTPS_PROXY / ALL_PROXY.</div></div>
              <Segmented label="Proxy mode" bind:value={s.proxyMode} options={[{ value: 'none', label: 'None' }, { value: 'system', label: 'System' }, { value: 'http', label: 'HTTP' }, { value: 'socks5', label: 'SOCKS5' }]} />
            </div>
            {#if s.proxyMode === 'http' || s.proxyMode === 'socks5'}
              <div class="grid2">
                <label class="field"><span>Host</span><input class="input mono" bind:value={s.proxyHost} /></label>
                <label class="field"><span>Port</span><input class="input mono" bind:value={s.proxyPort} inputmode="numeric" /></label>
                <label class="field"><span>Username (optional)</span><input class="input" bind:value={s.proxyUser} autocomplete="off" /></label>
                <label class="field"><span>Password</span><input class="input" type="password" bind:value={s.proxyPass} autocomplete="off" /></label>
              </div>
            {/if}
          </section>
        {:else if page === 'notifications'}
          <section>
            <h2>Notifications</h2>
            <div class="group card">
              <div class="rowc"><div class="t"><div class="tt">Download complete</div></div><Switch label="Notify on complete" bind:checked={s.notifyComplete} /></div>
              <div class="rowc"><div class="t"><div class="tt">Download failed</div></div><Switch label="Notify on failure" bind:checked={s.notifyFailure} /></div>
              <div class="rowc"><div class="t"><div class="tt">Queue finished</div></div><Switch label="Notify on queue finished" bind:checked={s.notifyQueueFinished} /></div>
            </div>
          </section>
        {:else if page === 'appearance'}
          <section>
            <h2>Appearance</h2>
            <div class="rowc card">
              <div class="t"><div class="tt">Theme</div><div class="d">Dark is the default.</div></div>
              <Segmented label="Theme" bind:value={s.theme} onchange={previewTheme} options={[{ value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }, { value: 'system', label: 'System' }]} />
            </div>
            <div class="rowc card">
              <div class="t"><div class="tt">List density</div><div class="d">Row height in the download list.</div></div>
              <Segmented label="List density" bind:value={s.density} options={[{ value: 'comfortable', label: 'Comfortable' }, { value: 'compact', label: 'Compact' }]} />
            </div>
            <label class="check"><input type="checkbox" bind:checked={s.traySpeed} />Show speed in the {isMac ? 'menu bar' : 'system tray'}</label>
          </section>
        {:else if page === 'advanced'}
          <section>
            <h2>Advanced</h2>
            <label class="field"><span>User agent</span><input class="input mono" bind:value={s.userAgent} spellcheck="false" /></label>
            <div class="grid2">
              <label class="field"><span>Retries per connection</span><input class="input mono" bind:value={s.maxRetries} inputmode="numeric" /></label>
              <label class="field"><span>Retry delay (seconds)</span><input class="input mono" bind:value={s.retryDelaySecs} inputmode="numeric" /></label>
              <label class="field"><span>Timeout (seconds)</span><input class="input mono" bind:value={s.timeoutSecs} inputmode="numeric" /></label>
              <label class="field"><span>Minimum segment size (KB)</span><input class="input mono" bind:value={s.minSegmentKb} inputmode="numeric" /></label>
              <label class="field"><span>Extension port</span><input class="input mono" bind:value={s.extensionPort} inputmode="numeric" /></label>
            </div>
            <div class="rowc card">
              <div class="t"><div class="tt">Verify TLS certificates</div><div class="d">Keep this on. Turning it off exposes downloads to tampering.</div></div>
              <Switch label="Verify TLS" bind:checked={s.verifyTls} />
            </div>
            {#if !s.verifyTls}<div class="errbox">Certificate checks are off. Anyone on the network path can alter what you download.</div>{/if}
          </section>
        {/if}
      {/if}
    </div>
    <div class="dialog-foot">
      <button class="btn" style="color:var(--text-muted)" onclick={restore}>Restore defaults</button>
      <span class="spacer"></span>
      <button class="btn" onclick={() => { if (base) previewTheme(base.theme); win.close(); }}>Cancel</button>
      <button class="btn primary" disabled={busy} onclick={() => save(true)}>Save</button>
    </div>
  </div>
</div>

<style>
  .wrap { height: 100%; display: flex; background: var(--surface); }
  nav { width: 220px; flex: none; border-right: 1px solid var(--border); background: #111418; padding: 16px 10px; display: flex; flex-direction: column; gap: 2px; overflow: auto; }
  :global([data-theme='light']) nav { background: var(--surface-2); }
  h1 { margin: 0 10px 12px; font-size: var(--fs-xl); font-weight: 600; }
  .nav { display: flex; align-items: center; gap: 9px; padding: 8px 10px; border-radius: 7px; border: 0; background: transparent; color: var(--text-muted); text-align: left; }
  .nav:hover { background: var(--surface-2); color: var(--text); }
  .nav.on { background: var(--accent-soft); color: var(--text); font-weight: 600; }
  .nav.on :global(svg) { color: var(--accent); }
  .content { flex: 1; min-width: 0; display: flex; flex-direction: column; }
  .scroll { flex: 1; overflow: auto; padding: 22px 28px; }
  section { display: flex; flex-direction: column; gap: 14px; }
  h2 { margin: 0; font-size: var(--fs-lg); font-weight: 600; }
  .rowc { display: flex; align-items: center; gap: 16px; padding: 14px 16px; }
  .card.rowc { border-radius: var(--r-lg); }
  .group { display: flex; flex-direction: column; overflow: hidden; }
  .group > .rowc + .rowc { border-top: 1px solid var(--border); }
  .t { flex: 1; min-width: 0; }
  .tt { font-weight: 500; }
  .d { font-size: var(--fs-sm); color: var(--text-muted); }
  .num { width: 28px; text-align: right; }
  .small-in { width: 70px; height: 32px; text-align: center; }
  .lim { width: 80px; height: 32px; text-align: right; }
  .lim.dim { color: var(--text-muted); }
  .unit { font-size: var(--fs-sm); }
  .small { font-size: var(--fs-sm); }
  .status { display: flex; align-items: center; gap: 6px; font-size: var(--fs-sm); }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--text-muted); opacity: .6; }
  .dot.ok { background: var(--success); opacity: 1; }
  .dot.bad { background: var(--danger); opacity: 1; }
  .bridge-err { color: var(--danger); }
  .path { flex: 2; min-width: 0; }
  .grid2 { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
  .catcard { padding: 12px 14px; display: flex; flex-direction: column; gap: 10px; }
  .cathead { display: flex; align-items: center; gap: 8px; position: relative; }
  .iconsel { position: absolute; left: 0; top: 0; width: 26px; height: 36px; opacity: 0; cursor: pointer; padding: 0; }
  .cathead > :global(svg) { color: var(--accent); pointer-events: none; width: 26px; }
  .catname { width: 170px; font-weight: 500; }
  .fold { display: flex; gap: 6px; min-width: 0; flex: 1; }
  .chips { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
  .chip { display: inline-flex; align-items: center; gap: 4px; height: 26px; padding: 0 4px 0 9px; border-radius: 7px; border: 1px solid var(--border); background: var(--surface-sunken); font-size: var(--fs-sm); }
  .chip button { display: flex; border: 0; background: transparent; color: var(--text-muted); padding: 3px; border-radius: 4px; }
  .chip button:hover { color: var(--danger); background: var(--danger-soft); }
  .chipin { height: 26px; width: 110px; border: 1px dashed var(--border); border-radius: 7px; background: transparent; padding: 0 8px; font-size: var(--fs-sm); outline: none; }
  .chipin:focus { border-color: var(--accent); border-style: solid; }
  .fold input { flex: 1; min-width: 0; }
  .btn.icon { width: 36px; padding: 0; }
</style>
