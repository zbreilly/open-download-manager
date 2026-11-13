<script lang="ts">
  // Theme studio: build, save, import and export colour profiles.
  // Whatever is selected here is previewed live in every open window; nothing
  // is kept until Save.
  import { onMount, untrack } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { save as saveDialog } from '@tauri-apps/plugin-dialog';
  import Icon from '../lib/Icon.svelte';
  import type { IconName } from '../lib/icons';
  import ColorTriangle from '../components/ColorTriangle.svelte';
  import Segmented from '../components/Segmented.svelte';
  import Switch from '../components/Switch.svelte';
  import { toast } from '../components/Toast.svelte';
  import { api, errText } from '../lib/ipc';
  import {
    ART_SLOTS, BUILTIN, BUTTON_SLOTS, COLOR_GROUPS, MAX_ASSET, applyProfile, isFontName, assetIds, assetUrl, collectAssets, defaultProfile, exportTheme, importTheme, legible,
    newProfileId, previewEverywhere, putAsset, sanitizeProfile, saveThemes, themeState,
    type ArtSlot, type ColorKey, type ThemeProfile,
  } from '../lib/themes.svelte';

  const win = getCurrentWindow();
  const TABS = [
    { id: 'colors', label: 'Colours' },
    { id: 'outlines', label: 'Outlines & shape' },
    { id: 'glass', label: 'Glass' },
    { id: 'fonts', label: 'Fonts' },
    { id: 'art', label: 'Panel art' },
    { id: 'buttons', label: 'Buttons' },
  ];
  // Shown until (or unless) the list of installed fonts arrives.
  const UI_FALLBACK = ['Segoe UI', 'Arial', 'Verdana', 'Tahoma', 'Trebuchet MS', 'Georgia', 'Times New Roman'];
  const MONO_FALLBACK = ['Cascadia Code', 'Consolas', 'Courier New'];
  /** Every font family installed on this computer (null: not loaded). */
  let installed = $state<{ name: string; mono: boolean }[] | null>(null);
  /** Options for a font field: the bundled font, the installed ones, and whatever the profile names. */
  function fontOptions(bundled: string, current: string, monoOnly: boolean) {
    const names = installed
      ? installed.filter((f) => isFontName(f.name) && (!monoOnly || f.mono)).map((f) => f.name)
      : monoOnly ? MONO_FALLBACK : UI_FALLBACK;
    const seen = new Set<string>();
    return [bundled, ...names, current].filter((n) => n && !seen.has(n.toLowerCase()) && seen.add(n.toLowerCase()));
  }

  const osBase = (): 'dark' | 'light' => (document.documentElement.dataset.theme === 'light' ? 'light' : 'dark');
  const clone = <T,>(x: T): T => JSON.parse(JSON.stringify(x));

  let tab = $state('colors');
  let sel = $state<ColorKey>('accent');
  /** The user's own profiles, including ones not saved yet. */
  let custom = $state<ThemeProfile[]>([]);
  let draft = $state<ThemeProfile>(defaultProfile('dark'));
  let pristine = '';
  let savedJson = $state('[]');
  let savedActive = $state('');
  let ready = $state(false);
  let busy = $state(false);
  let assetTick = $state(0);
  const uiFonts = $derived(fontOptions('IBM Plex Sans', draft.fonts.ui, false));
  const monoFonts = $derived(fontOptions('JetBrains Mono', draft.fonts.mono, true));
  /** Number font: fixed-width fonts only, unless the user asks for all. */
  let allForNumbers = $state(false);
  const numberFonts = $derived(allForNumbers ? fontOptions('JetBrains Mono', draft.fonts.mono, false) : monoFonts);

  const isDefault = $derived(draft.builtin === true && draft.id === 'default');
  const choice = $derived(isDefault ? '' : draft.id);
  const dirty = $derived(JSON.stringify(custom) !== savedJson || choice !== savedActive);
  const selLabel = $derived(COLOR_GROUPS.flatMap((g) => g.keys).find((k) => k.key === sel)?.label ?? '');
  const palette = $derived([...new Set(Object.values(draft.colors))].slice(0, 24));
  /** Labels of the colours the app replaces because they could not be read. */
  const unreadable = $derived(legible(draft.colors).fixed.map((k) => COLOR_GROUPS.flatMap((g) => g.keys).find((x) => x.key === k)?.label ?? k));

  function select(p: ThemeProfile) {
    draft = p.builtin ? clone(p) : (custom.find((c) => c.id === p.id) ?? clone(p));
    pristine = JSON.stringify(draft);
  }
  function selectId(id: string) {
    const p = id ? BUILTIN.find((b) => b.id === id) ?? custom.find((c) => c.id === id) : null;
    select(p ?? defaultProfile(osBase()));
  }
  function uniqueName(name: string) {
    const taken = new Set([...BUILTIN, ...custom].map((p) => p.name));
    if (!taken.has(name)) return name;
    for (let i = 2; ; i++) if (!taken.has(`${name} ${i}`)) return `${name} ${i}`;
  }

  // Every change to the draft: a built-in that was edited becomes the user's
  // own copy, and all windows show the result.
  let timer: ReturnType<typeof setTimeout>;
  $effect(() => {
    const snap = JSON.stringify(draft);
    if (!ready) return;
    untrack(() => {
      if (draft.builtin && snap !== pristine) {
        draft.builtin = false;
        draft.id = newProfileId();
        draft.name = uniqueName(`${draft.name} (custom)`);
        custom.push(draft);
        draft = custom[custom.length - 1];
        pristine = JSON.stringify(draft);
        return; // the changes above re-run this effect
      }
      clearTimeout(timer);
      timer = setTimeout(async () => {
        const p = isDefault ? null : sanitizeProfile(JSON.parse(snap));
        if (p) p.builtin = draft.builtin;
        await applyProfile(p);
        assetTick++;
        previewEverywhere(p).catch(() => {});
      }, 40);
    });
  });

  async function save() {
    busy = true;
    try {
      const list = $state.snapshot(custom) as ThemeProfile[];
      await saveThemes(list, choice);
      savedJson = JSON.stringify(list);
      savedActive = choice;
      toast(`“${draft.name}” is now in use`);
    } catch (e) {
      toast(errText(e), 'error');
    } finally {
      busy = false;
    }
  }
  async function close() {
    // drop the preview: every window goes back to what is saved
    await api.broadcast('theme_changed', null).catch(() => {});
    win.close();
  }
  function duplicate() {
    const c = clone($state.snapshot(draft) as ThemeProfile);
    c.builtin = false;
    c.id = newProfileId();
    c.name = uniqueName(`${draft.name} copy`);
    custom.push(c);
    select(c);
  }
  function remove() {
    if (draft.builtin) return;
    const i = custom.findIndex((c) => c.id === draft.id);
    if (i >= 0) custom.splice(i, 1);
    selectId('');
  }

  // ---------------- files ----------------
  function pickFile(accept: string): Promise<File | null> {
    return new Promise((resolve) => {
      const input = document.createElement('input');
      input.type = 'file';
      input.accept = accept;
      input.onchange = () => resolve(input.files?.[0] ?? null);
      input.oncancel = () => resolve(null);
      input.click();
    });
  }
  const MIME: Record<string, string> = {
    png: 'image/png', svg: 'image/svg+xml', ttf: 'font/ttf', otf: 'font/otf', woff: 'font/woff', woff2: 'font/woff2',
  };
  /** Read a picked file into the asset store; returns its id. */
  async function importAsset(accept: string[]): Promise<string | null> {
    const file = await pickFile(accept.map((e) => '.' + e).join(','));
    if (!file) return null;
    const ext = file.name.split('.').pop()?.toLowerCase() ?? '';
    if (!accept.includes(ext)) throw new Error(`Choose a ${accept.map((e) => '.' + e).join(' or ')} file`);
    if (file.size * 1.34 > MAX_ASSET) throw new Error('That file is too large (limit 6 MB)');
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (ext === 'png' && !(bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47)) throw new Error('That is not a PNG image');
    if (ext === 'svg' && !new TextDecoder().decode(bytes.slice(0, 4096)).includes('<svg')) throw new Error('That is not an SVG image');
    let bin = '';
    for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
    return putAsset(`data:${MIME[ext]};base64,${btoa(bin)}`);
  }
  async function act(fn: () => Promise<unknown>) {
    try { await fn(); } catch (e) { toast(errText(e), 'error'); }
  }
  const IMAGES = ['png', 'svg'];
  const SKINS: { id: 'button' | 'primary'; label: string }[] = [{ id: 'button', label: 'Ordinary buttons' }, { id: 'primary', label: 'Primary buttons' }];
  const FONTS = ['ttf', 'otf', 'woff', 'woff2'];

  const setArt = (slot: ArtSlot) => act(async () => {
    const id = await importAsset(IMAGES);
    if (id) draft.art[slot] = { asset: id, fit: draft.art[slot]?.fit ?? 'cover', tint: draft.art[slot]?.tint ?? 0.55 };
  });
  const setIcon = (name: string) => act(async () => {
    const id = await importAsset(IMAGES);
    if (id) draft.icons[name] = { asset: id, tint: draft.icons[name]?.tint ?? false };
  });
  const setSkin = (which: 'button' | 'primary') => act(async () => {
    const id = await importAsset(IMAGES);
    if (id) draft.skin[which] = id;
  });
  const setFont = (which: 'uiAsset' | 'monoAsset') => act(async () => {
    const id = await importAsset(FONTS);
    if (id) draft.fonts[which] = id;
  });

  const doExport = () => act(async () => {
    const text = await exportTheme($state.snapshot(draft) as ThemeProfile);
    const safe = draft.name.replace(/[\\/:*?"<>|]/g, '_').trim() || 'theme';
    const path = await saveDialog({ defaultPath: `${safe}.odmtheme`, filters: [{ name: 'Open Download Manager theme', extensions: ['odmtheme'] }] });
    if (!path) return;
    await api.writeThemeFile(path, text);
    toast('Theme exported');
  });
  const doImport = () => act(async () => {
    const file = await pickFile('.odmtheme,.json');
    if (!file) return;
    if (file.size > 80_000_000) throw new Error('That file is too large to be a theme');
    const p = await importTheme(await file.text());
    p.name = uniqueName(p.name);
    custom.push(p);
    select(p);
    toast(`Imported “${p.name}” — press Save to keep it`);
  });

  onMount(() => {
    // Root has already loaded the saved profiles into themeState
    custom = clone($state.snapshot(themeState.profiles) as ThemeProfile[]);
    savedJson = JSON.stringify(custom);
    savedActive = themeState.activeId;
    selectId(savedActive);
    ready = true;
    // nothing is being edited yet: files left by an import or a picture
    // that was never saved can go
    collectAssets(themeState.profiles.flatMap(assetIds)).catch(() => {});
    api.listFonts().then((list) => (installed = list)).catch(() => (installed = []));
    const k = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !(e.target as HTMLElement)?.closest?.('input, select')) close();
      if ((e.ctrlKey || e.metaKey) && e.key === 's') { e.preventDefault(); save(); }
    };
    window.addEventListener('keydown', k);
    return () => window.removeEventListener('keydown', k);
  });
</script>

{#snippet swatches(title: string)}
  {#each COLOR_GROUPS.filter((g) => g.title === title) as g}
    <div class="grp">
      <div class="label">{g.title}</div>
      {#each g.keys as k}
        <button class="sw" class:on={sel === k.key} onclick={() => (sel = k.key)}>
          <span class="chip" style:background={draft.colors[k.key]}></span>
          <span class="nm">{k.label}{#if k.hint}<span class="muted small"> · {k.hint}</span>{/if}</span>
          <span class="mono small muted">{draft.colors[k.key]}</span>
        </button>
      {/each}
    </div>
  {/each}
{/snippet}

{#snippet slider(label: string, hint: string, get: () => number, set: (v: number) => void, min: number, max: number, step: number, unit: string)}
  <div class="rowc card">
    <div class="t"><div class="tt">{label}</div><div class="d">{hint}</div></div>
    <input type="range" {min} {max} {step} value={get()} oninput={(e) => set(Number(e.currentTarget.value))} aria-label={label} />
    <span class="mono num">{unit === '%' ? Math.round(get() * 100) + '%' : unit === '×' ? get().toFixed(2) + '×' : get() + unit}</span>
  </div>
{/snippet}

<div class="studio">
  <aside class="rail glass-chrome">
    <h1>Theme studio</h1>
    <div class="label">Built in</div>
    <button class="pi" class:on={isDefault} onclick={() => selectId('')}>
      <span class="dots"><i style:background="#06080C"></i><i style:background="#D3DAE8"></i><i style:background="#5B9DFF"></i></span>
      <span class="pn">Default</span>{#if savedActive === ''}<span class="use">in use</span>{/if}
    </button>
    {#each BUILTIN as p (p.id)}
      <button class="pi" class:on={draft.builtin && draft.id === p.id} onclick={() => select(p)}>
        <span class="dots"><i style:background={p.colors.window}></i><i style:background={p.colors.outlinePanel}></i><i style:background={p.colors.accent}></i></span>
        <span class="pn">{p.name}</span>{#if savedActive === p.id}<span class="use">in use</span>{/if}
      </button>
    {/each}
    <div class="label mine">Your profiles</div>
    {#each custom as p (p.id)}
      <button class="pi" class:on={!draft.builtin && draft.id === p.id} onclick={() => select(p)}>
        <span class="dots"><i style:background={p.colors.window}></i><i style:background={p.colors.outlinePanel}></i><i style:background={p.colors.accent}></i></span>
        <span class="pn">{p.name}</span>{#if savedActive === p.id}<span class="use">in use</span>{/if}
      </button>
    {:else}
      <div class="muted small none">Change anything on a built-in profile and it becomes your own copy.</div>
    {/each}
    <span class="spacer"></span>
    <div class="railbtns">
      <button class="btn sm c-green" onclick={duplicate}><Icon name="plus" size={14} />New from this</button>
      <button class="btn sm c-teal" onclick={doImport}><Icon name="import" size={14} />Import…</button>
      <button class="btn sm c-orange" onclick={doExport}><Icon name="export" size={14} />Export…</button>
      <button class="btn sm c-red" disabled={draft.builtin} onclick={remove}><Icon name="delete" size={14} />Delete</button>
    </div>
  </aside>

  <div class="work art-dialog">
    <div class="top">
      <input class="input name" bind:value={draft.name} aria-label="Profile name" maxlength="60" disabled={isDefault} />
      <div class="tabs" role="tablist">
        {#each TABS as t}
          <button role="tab" aria-selected={tab === t.id} class:on={tab === t.id} onclick={() => (tab = t.id)}>{t.label}</button>
        {/each}
      </div>
    </div>

    <div class="mid">
      <div class="pane">
        {#if isDefault}
          <div class="note">Default follows the Light / Dark choice in Settings. Change anything here to start your own profile from it.</div>
        {/if}
        {#if tab === 'colors'}
          {@render swatches('Windows & layers')}
          {@render swatches('Text')}
          {@render swatches('Accent & status')}
        {:else if tab === 'outlines'}
          {@render swatches('Outlines')}
          {@render slider('Window outline', 'A frame around the inside edge of every window.', () => draft.outline.window, (v) => (draft.outline.window = v), 0, 8, 1, ' px')}
          {@render slider('Layer outline', 'Panels, the sidebar and header edges, menus and dialogs.', () => draft.outline.panel, (v) => (draft.outline.panel = v), 0, 4, 0.5, ' px')}
          {@render slider('Control outline', 'Buttons, text fields and cards.', () => draft.outline.control, (v) => (draft.outline.control = v), 0, 3, 0.5, ' px')}
          {@render slider('Neon glow', 'Light spilling from the window and layer outlines.', () => draft.outline.neon, (v) => (draft.outline.neon = v), 0, 30, 1, ' px')}
          {@render slider('Corner roundness', '0 is square.', () => draft.outline.radius, (v) => (draft.outline.radius = v), 0, 2, 0.05, '×')}
        {:else if tab === 'glass'}
          <div class="rowc card">
            <div class="t"><div class="tt">Built on</div><div class="d">Decides scrollbars, native menus and other parts the app does not draw itself.</div></div>
            <Segmented label="Built on" bind:value={draft.base} options={[{ value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }]} />
          </div>
          {@render slider('Panel opacity', 'Lower lets the window background show through panels.', () => draft.panelOpacity, (v) => (draft.panelOpacity = v), 0.2, 1, 0.01, '%')}
          {@render slider('Header & sidebar opacity', '', () => draft.chromeOpacity, (v) => (draft.chromeOpacity = v), 0.2, 1, 0.01, '%')}
          {@render slider('Blur', 'How much panels blur what is behind them.', () => draft.blur, (v) => (draft.blur = v), 0, 40, 1, ' px')}
          {@render slider('Background glow', 'Strength of the two colour glows behind everything.', () => draft.glow, (v) => (draft.glow = v), 0, 1, 0.01, '%')}
          <div class="rowc card">
            <div class="t"><div class="tt">Flat</div><div class="d">No highlights or drop shadows.</div></div>
            <Switch label="Flat" bind:checked={draft.flat} />
          </div>
          {@render swatches('Background glow')}
        {:else if tab === 'fonts'}
          <div class="rowc card">
            <div class="t"><div class="tt">Interface font</div><div class="d">{installed ? `${installed.length} fonts installed on this computer.` : 'Loading the fonts installed on this computer…'}</div></div>
            <select class="select fnt" bind:value={draft.fonts.ui} disabled={!!draft.fonts.uiAsset} aria-label="Interface font">
              {#each uiFonts as f (f)}<option value={f}>{f}</option>{/each}
            </select>
          </div>
          <div class="rowc card">
            <div class="t"><div class="tt">Interface font file</div><div class="d">{draft.fonts.uiAsset ? 'An imported font file is in use; it travels with the profile when you export.' : 'Use a .ttf, .otf, .woff or .woff2 file instead of an installed font.'}</div></div>
            {#if draft.fonts.uiAsset}<button class="btn sm c-red" onclick={() => (draft.fonts.uiAsset = undefined)}>Remove</button>{/if}
            <button class="btn sm" onclick={() => setFont('uiAsset')}><Icon name="import" size={14} />Import font…</button>
          </div>
          <div class="rowc card">
            <div class="t"><div class="tt">Number font</div><div class="d">Sizes, speeds, percentages, paths and links. Fixed-width fonts keep columns of numbers lined up.</div>
              <label class="check small allf"><input type="checkbox" bind:checked={allForNumbers} />Show all fonts</label></div>
            <select class="select fnt" bind:value={draft.fonts.mono} disabled={!!draft.fonts.monoAsset} aria-label="Number font">
              {#each numberFonts as f (f)}<option value={f}>{f}</option>{/each}
            </select>
          </div>
          <div class="rowc card">
            <div class="t"><div class="tt">Number font file</div><div class="d">{draft.fonts.monoAsset ? 'An imported font file is in use.' : 'Optional font file for numbers.'}</div></div>
            {#if draft.fonts.monoAsset}<button class="btn sm c-red" onclick={() => (draft.fonts.monoAsset = undefined)}>Remove</button>{/if}
            <button class="btn sm" onclick={() => setFont('monoAsset')}><Icon name="import" size={14} />Import font…</button>
          </div>
          {@render slider('Text size', '', () => draft.fonts.scale, (v) => (draft.fonts.scale = v), 0.85, 1.3, 0.01, '%')}
          <div class="sample card">
            <div class="big">The quick brown fox jumps over the lazy dog</div>
            <div class="mono">0123456789 · 18.4 MB/s · 46% · C:\Users\me\Downloads</div>
          </div>
        {:else if tab === 'art'}
          <p class="muted small lead">Put a .png or .svg behind any panel. “Colour wash” lays the panel’s own colour over the picture so text stays readable.</p>
          {#each ART_SLOTS as slot (slot.id)}
            {@const a = draft.art[slot.id]}
            <div class="artrow card">
              <div class="thumb" style:background-image={a && assetTick >= 0 && assetUrl(a.asset) ? `url("${assetUrl(a.asset)}")` : 'none'}>{#if !a}<Icon name="file-image" size={20} />{/if}</div>
              <div class="t">
                <div class="tt">{slot.label}</div>
                {#if a}
                  <div class="artctl">
                    <select class="select" bind:value={a.fit} aria-label="How the picture fits {slot.label}">
                      <option value="cover">Fill</option><option value="contain">Fit</option><option value="stretch">Stretch</option><option value="tile">Tile</option>
                    </select>
                    <label class="wash">Colour wash<input type="range" min="0" max="0.95" step="0.01" bind:value={a.tint} /><span class="mono small">{Math.round(a.tint * 100)}%</span></label>
                  </div>
                {:else}
                  <div class="d">No picture</div>
                {/if}
              </div>
              {#if a}<button class="btn sm c-red" onclick={() => delete draft.art[slot.id]}>Remove</button>{/if}
              <button class="btn sm" onclick={() => setArt(slot.id)}><Icon name="import" size={14} />{a ? 'Replace…' : 'Choose picture…'}</button>
            </div>
          {/each}
        {:else if tab === 'buttons'}
          <p class="muted small lead">Replace a button’s icon with your own .png or .svg. It changes everywhere that icon appears, menus included. “Tint” draws a single-colour picture in the button’s own colour.</p>
          <div class="btngrid">
            {#each BUTTON_SLOTS as b (b.icon)}
              {@const ov = draft.icons[b.icon]}
              <div class="bcell card" class:has={!!ov}>
                <div class="bprev"><Icon name={b.icon as IconName} size={22} /></div>
                <div class="bname">{b.label}</div>
                <div class="bact">
                  <button class="btn link small" onclick={() => setIcon(b.icon)}>{ov ? 'Replace' : 'Import'}</button>
                  {#if ov}
                    <label class="check small"><input type="checkbox" bind:checked={ov.tint} />Tint</label>
                    <button class="btn link small" onclick={() => delete draft.icons[b.icon]}>Reset</button>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
          <div class="label skinlbl">Button skins</div>
          {#each SKINS as sk (sk.id)}
            <div class="rowc card">
              <div class="t"><div class="tt">{sk.label}</div><div class="d">{draft.skin[sk.id] ? 'A picture is stretched across each button in place of its fill and outline.' : 'Optional picture used as the button background.'}</div></div>
              <button class={sk.id === 'primary' ? 'btn primary' : 'btn'}>Sample</button>
              {#if draft.skin[sk.id]}<button class="btn sm c-red" onclick={() => (draft.skin[sk.id] = undefined)}>Remove</button>{/if}
              <button class="btn sm" onclick={() => setSkin(sk.id)}><Icon name="import" size={14} />Import…</button>
            </div>
          {/each}
        {/if}
      </div>

      <aside class="pick">
        <div class="label">Editing</div>
        <div class="ptitle">{selLabel}</div>
        <ColorTriangle bind:value={draft.colors[sel]} />
        <div class="label">In this profile</div>
        <div class="pal">
          {#each palette as c}
            <button class="pchip" style:background={c} title={c} aria-label="Use {c}" onclick={() => (draft.colors[sel] = c)}></button>
          {/each}
        </div>
        {#if unreadable.length}
          <div class="fixnote" role="status">{unreadable.join(', ')}: too close to a background colour to read, so the app shows {unreadable.length > 1 ? 'them' : 'it'} in black or white instead.</div>
        {/if}
      </aside>
    </div>

    <div class="dialog-foot">
      <span class="muted small">{dirty ? 'Previewing in every window — not saved yet.' : 'Saved.'}</span>
      <span class="spacer"></span>
      <button class="btn" onclick={close}>{dirty ? 'Cancel' : 'Close'}</button>
      <button class="btn primary" disabled={busy || !dirty} onclick={save}>Save & use</button>
    </div>
  </div>
</div>

<style>
  .studio { height: 100%; display: flex; }
  .small { font-size: var(--fs-sm); }
  .rail { width: 236px; flex: none; border-right: var(--outline-w, 1px) solid var(--glass-border); padding: 16px 10px 12px; display: flex; flex-direction: column; gap: 2px; overflow: auto; }
  h1 { margin: 0 8px 12px; font-size: var(--fs-xl); font-weight: 600; }
  .rail .label { padding: 0 8px 6px; }
  .rail .mine { margin-top: 14px; }
  .none { padding: 2px 8px; }
  .pi { display: flex; align-items: center; gap: 9px; padding: 7px 8px; border-radius: var(--r-md); border: 0; background: transparent; color: var(--text-muted); text-align: left; }
  .pi:hover { background: var(--hover); color: var(--text); }
  .pi.on { background: var(--accent-soft); color: var(--text); font-weight: 600; box-shadow: inset 3px 0 0 var(--accent); }
  .pn { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .use { font-size: 10px; font-weight: 600; text-transform: uppercase; letter-spacing: .05em; color: var(--green); background: var(--green-soft); padding: 1px 6px; border-radius: var(--r-pill); }
  .dots { display: flex; flex: none; }
  .dots i { width: 13px; height: 13px; border-radius: 50%; border: 1px solid var(--text-muted); margin-left: -4px; }
  .dots i:first-child { margin-left: 0; }
  .railbtns { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; padding-top: 10px; }
  .railbtns .btn { padding: 0 6px; font-size: var(--fs-sm); }

  .work { flex: 1; min-width: 0; display: flex; flex-direction: column; background: var(--glass); -webkit-backdrop-filter: var(--blur); backdrop-filter: var(--blur); }
  .top { display: flex; align-items: center; gap: 14px; padding: 12px 18px; border-bottom: 1px solid var(--border); flex-wrap: wrap; }
  .name { width: 230px; font-weight: 600; }
  .tabs { display: flex; gap: 2px; flex-wrap: wrap; }
  .tabs button { height: 32px; padding: 0 12px; border: 0; border-radius: var(--r-md); background: transparent; color: var(--text-muted); }
  .tabs button:hover { background: var(--hover); color: var(--text); }
  .tabs button.on { background: var(--accent-soft); color: var(--accent); font-weight: 600; }

  .mid { flex: 1; min-height: 0; display: flex; }
  .pane { flex: 1; min-width: 0; overflow: auto; padding: 16px 18px; display: flex; flex-direction: column; gap: 10px; }
  .note { padding: 9px 12px; border-radius: var(--r-md); background: var(--accent-soft); font-size: var(--fs-sm); }
  .lead { margin: 0; }
  .grp { display: flex; flex-direction: column; gap: 2px; margin-bottom: 8px; }
  .grp .label { padding: 0 0 4px; }
  .sw { display: flex; align-items: center; gap: 10px; padding: 6px 8px; border-radius: var(--r-md); border: 1px solid transparent; background: transparent; color: var(--text); text-align: left; }
  .sw:hover { background: var(--hover); }
  .sw.on { background: var(--accent-soft); border-color: var(--accent); }
  .chip { width: 30px; height: 22px; border-radius: 5px; border: 1px solid var(--text-muted); flex: none; }
  .nm { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .rowc { display: flex; align-items: center; gap: 14px; padding: 12px 14px; }
  .t { flex: 1; min-width: 0; }
  .tt { font-weight: 500; }
  .d { font-size: var(--fs-sm); color: var(--text-muted); }
  .rowc input[type='range'] { width: 170px; }
  .num { width: 58px; text-align: right; font-size: var(--fs-sm); }
  .fnt { width: 240px; }
  .allf { margin-top: 6px; gap: 6px; color: var(--text-muted); }
  .allf input { width: 13px; height: 13px; }
  .sample { padding: 14px; display: flex; flex-direction: column; gap: 6px; }
  .big { font-size: var(--fs-xl); font-weight: 600; }

  .artrow { display: flex; align-items: center; gap: 12px; padding: 10px 12px; }
  .thumb { width: 72px; height: 48px; flex: none; border-radius: var(--r-md); border: 1px solid var(--border); background: var(--surface-sunken) center / cover no-repeat; display: flex; align-items: center; justify-content: center; color: var(--text-muted); }
  .artctl { display: flex; align-items: center; gap: 12px; margin-top: 6px; flex-wrap: wrap; }
  .artctl .select { height: 30px; width: 100px; }
  .wash { display: flex; align-items: center; gap: 8px; font-size: var(--fs-sm); color: var(--text-muted); }
  .wash input { width: 110px; }

  .btngrid { display: grid; grid-template-columns: repeat(auto-fill, minmax(128px, 1fr)); gap: 8px; }
  .bcell { padding: 10px; display: flex; flex-direction: column; align-items: center; gap: 6px; text-align: center; }
  .bcell.has { border-color: var(--accent); }
  .bprev { width: 44px; height: 44px; border-radius: var(--r-md); background: var(--surface-sunken); border: 1px solid var(--border); display: flex; align-items: center; justify-content: center; }
  .bname { font-size: var(--fs-sm); font-weight: 500; }
  .bact { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; justify-content: center; min-height: 20px; }
  .bact .check { gap: 5px; }
  .bact input { width: 13px; height: 13px; }
  .skinlbl { margin-top: 10px; }

  .pick { width: 268px; flex: none; border-left: 1px solid var(--border); padding: 16px 18px; display: flex; flex-direction: column; gap: 8px; overflow: auto; background: var(--tint); }
  .ptitle { font-weight: 600; font-size: var(--fs-lg); margin-bottom: 4px; }
  .pick .label:not(:first-child) { margin-top: 8px; }
  .pal { display: flex; flex-wrap: wrap; gap: 5px; }
  .pchip { width: 22px; height: 22px; border-radius: 5px; border: 1px solid var(--text-muted); padding: 0; }
  .pchip:hover { transform: scale(1.15); }
  .fixnote { margin-top: 6px; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--r-md); font-size: var(--fs-sm); color: var(--text); }
</style>
