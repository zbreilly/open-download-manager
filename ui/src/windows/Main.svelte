<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
  import Icon from '../lib/Icon.svelte';
  import { ICONS, type IconName } from '../lib/icons';
  import { api, errText, on, type Download, type PowerCountdown } from '../lib/ipc';
  import { store, initStore, byId, selection } from '../lib/store.svelte';
  import * as f from '../lib/format';
  import { modKey, isMac } from '../lib/theme';
  import StatusPill from '../components/StatusPill.svelte';
  import FileBadge from '../components/FileBadge.svelte';
  import SegmentMap from '../components/SegmentMap.svelte';
  import Sparkline from '../components/Sparkline.svelte';
  import ContextMenu, { type MenuItem } from '../components/ContextMenu.svelte';
  import Modal from '../components/Modal.svelte';
  import { toast } from '../components/Toast.svelte';

  type Filter = { kind: 'lib' | 'cat' | 'queue'; id: string };
  let filter = $state<Filter>({ kind: 'lib', id: 'all' });
  let searchInput = $state('');
  let search = $state('');
  let searchEl = $state<HTMLInputElement | undefined>();
  const selected = selection;
  let focusedId = $state<string | null>(null);
  let anchorId = $state<string | null>(null);
  let menu: { x: number; y: number; items: MenuItem[] } | null = $state(null);
  let confirmDelete: { ids: string[]; files: boolean } | null = $state(null);
  let refreshModal: { id: string; url: string; busy: boolean; err: string } | null = $state(null);
  let limitModal = $state<{ value: string } | null>(null);
  let power: { action: string; queue: string; token: string | null; left: number } | null = $state(null);
  let dropOver = $state(false);
  let dragTarget = $state<string | null>(null);

  // ---------------- columns ----------------
  type ColKey = 'name' | 'size' | 'progress' | 'speed' | 'eta' | 'status' | 'queue' | 'added';
  type Col = { key: ColKey; label: string; width: number; visible: boolean; align?: 'right' };
  const DEFAULT_COLS: Col[] = [
    { key: 'name', label: 'Name', width: 240, visible: true },
    { key: 'size', label: 'Size', width: 92, visible: true },
    { key: 'progress', label: 'Progress', width: 164, visible: true },
    { key: 'speed', label: 'Speed', width: 104, visible: true, align: 'right' },
    { key: 'eta', label: 'ETA', width: 72, visible: true, align: 'right' },
    { key: 'status', label: 'Status', width: 140, visible: true },
    { key: 'queue', label: 'Queue', width: 96, visible: true },
    { key: 'added', label: 'Added', width: 112, visible: true },
  ];
  let cols = $state<Col[]>(structuredClone(DEFAULT_COLS));
  let sort = $state({ key: 'added' as ColKey, dir: -1 });
  const visibleCols = $derived(cols.filter((c) => c.visible));
  let viewW = $state(1000);
  // Name absorbs spare width so the table always fills the panel; every
  // column (Name included) can still be dragged wider, which scrolls.
  const othersWidth = $derived(visibleCols.reduce((a, c) => a + (c.key === 'name' ? 0 : c.width), 0));
  const colWidth = (c: Col) => (c.key === 'name' ? Math.max(c.width, viewW - 2 - othersWidth) : c.width);
  const tableWidth = $derived(visibleCols.reduce((a, c) => a + colWidth(c), 0));

  function persistCols() {
    api.kvSet('columns', JSON.stringify({ cols, sort })).catch(() => {});
  }

  // ---------------- data views ----------------
  const isActive = (d: Download) => d.status === 'downloading' || d.status === 'connecting' || d.status === 'assembling';
  function inFilter(d: Download, fl: Filter) {
    if (fl.kind === 'lib') {
      if (fl.id === 'active') return isActive(d);
      if (fl.id === 'unfinished') return d.status !== 'complete';
      if (fl.id === 'finished') return d.status === 'complete';
      return true;
    }
    if (fl.kind === 'cat') return d.category === fl.id;
    return d.queueId === fl.id;
  }
  const queueName = (id: string | null) => (id ? store.queues.find((q) => q.id === id)?.name ?? '—' : '—');

  const rows = $derived.by(() => {
    const s = search.trim().toLowerCase();
    let list = store.downloads.filter((d) => inFilter(d, filter));
    if (s) list = list.filter((d) =>
      d.fileName.toLowerCase().includes(s) || d.url.toLowerCase().includes(s) || d.saveDir.toLowerCase().includes(s));
    const dir = sort.dir;
    const key = sort.key;
    const val = (d: Download): number | string => {
      switch (key) {
        case 'name': return d.fileName.toLowerCase();
        case 'size': return d.totalBytes ?? -1;
        case 'progress': return d.totalBytes ? d.receivedBytes / d.totalBytes : d.status === 'complete' ? 1 : 0;
        case 'speed': return d.speedBps;
        case 'eta': return d.etaSecs ?? Number.MAX_SAFE_INTEGER;
        case 'status': return d.status;
        case 'queue': return queueName(d.queueId);
        case 'added': return d.addedAt;
      }
    };
    return [...list].sort((a, b) => {
      const x = val(a), y = val(b);
      return (x < y ? -1 : x > y ? 1 : 0) * dir;
    });
  });

  const focused = $derived((focusedId && byId(focusedId) && rows.find((r) => r.id === focusedId)) || rows[0] || null);
  const selectedRows = $derived(store.downloads.filter((d) => selected.has(d.id)));
  const canResume = $derived(selectedRows.some((d) => d.status === 'paused' || d.status === 'error' || d.status === 'queued'));
  const canPause = $derived(selectedRows.some((d) => isActive(d) || d.status === 'queued'));

  const counts = $derived.by(() => {
    const c: Record<string, number> = { all: 0, active: 0, unfinished: 0, finished: 0 };
    for (const d of store.downloads) {
      c.all++;
      if (isActive(d)) c.active++;
      if (d.status === 'complete') c.finished++; else c.unfinished++;
      c['cat:' + d.category] = (c['cat:' + d.category] ?? 0) + 1;
      if (d.queueId) c['q:' + d.queueId] = (c['q:' + d.queueId] ?? 0) + 1;
    }
    return c;
  });

  // ---------------- virtualization ----------------
  let scroller = $state<HTMLDivElement | undefined>();
  let scrollTop = $state(0);
  let viewH = $state(600);
  const rowH = $derived(store.settings?.density === 'compact' ? 30 : 40);
  const virtual = $derived(rows.length > 200);
  const vStart = $derived(virtual ? Math.max(0, Math.floor(scrollTop / rowH) - 10) : 0);
  const vEnd = $derived(virtual ? Math.min(rows.length, Math.ceil((scrollTop + viewH) / rowH) + 10) : rows.length);
  const shown = $derived(rows.slice(vStart, vEnd));

  // ---------------- actions ----------------
  const ids = () => [...selected];
  function targetIds(d?: Download) {
    if (d && !selected.has(d.id)) return [d.id];
    const s = ids();
    return s.length ? s : focused ? [focused.id] : [];
  }
  async function act(fn: () => Promise<unknown>) {
    try { await fn(); } catch (e) { toast(errText(e), 'error'); }
  }
  const resumeSel = (list = targetIds()) => act(() => api.resume(list));
  const pauseSel = (list = targetIds()) => act(() => api.pause(list));
  function askDelete(list = targetIds()) {
    if (!list.length) return;
    if (store.settings?.confirmDelete === false) return void act(() => api.delete(list, false));
    confirmDelete = { ids: list, files: false };
  }
  async function doDelete() {
    if (!confirmDelete) return;
    const { ids: list, files } = confirmDelete;
    confirmDelete = null;
    await act(() => api.delete(list, files));
    for (const i of list) selected.delete(i);
  }
  function addUrl(params?: unknown) { act(() => api.openWindow('add', null, params)); }
  function openProgress(d: Download) { act(() => api.openWindow('progress', d.id)); }
  function openItem(d: Download) {
    if (d.status === 'complete') act(() => api.openFile(d.id));
    else openProgress(d);
  }
  function copyLink(d: Download) {
    act(async () => {
      await writeText(d.url);
      toast('Link copied');
    });
  }
  function currentQueueId() {
    return filter.kind === 'queue' ? filter.id : 'main';
  }
  const limitOn = $derived(store.settings?.globalLimitEnabled ?? false);
  const limitMB = $derived(Math.round(((store.settings?.globalLimitBps ?? 0) / f.MBPS) * 10) / 10);
  function toggleLimit() {
    act(() => api.setGlobalLimit(limitOn ? null : store.settings?.globalLimitBps ?? 0));
  }
  async function saveLimit() {
    const bps = f.parseLimitMB(limitModal?.value ?? '');
    if (bps == null) return toast(f.LIMIT_ERROR, 'error');
    limitModal = null;
    await act(() => api.setGlobalLimit(bps));
  }
  async function doRefresh() {
    if (!refreshModal) return;
    refreshModal.busy = true;
    refreshModal.err = '';
    try {
      await api.refreshLink(refreshModal.id, refreshModal.url.trim());
      refreshModal = null;
      toast('Link updated');
    } catch (e) {
      if (refreshModal) { refreshModal.err = errText(e); refreshModal.busy = false; }
    }
  }
  function importList() {
    act(async () => {
      const p = await openDialog({ multiple: false, filters: [{ name: 'Text', extensions: ['txt', 'csv', 'lst'] }] });
      if (!p) return;
      const urls = await api.readUrlList(p as string);
      if (!urls.length) return toast('No http(s) links found in that file', 'error');
      await api.openWindow('grabber', null, { links: urls.map((url) => ({ url })) });
    });
  }
  function exportList() {
    act(async () => {
      const p = await saveDialog({ defaultPath: 'downloads.txt', filters: [{ name: 'Text', extensions: ['txt'] }] });
      if (!p) return;
      const n = await api.exportList(p, false);
      toast(`Exported ${n} links`);
    });
  }

  function fileExt(d: Download) {
    const i = d.fileName.lastIndexOf('.');
    return i > 0 && d.fileName.length - i <= 12 ? d.fileName.slice(i + 1).toLowerCase() : '';
  }
  function catForExt(ext: string) {
    const cats = store.settings?.categories ?? [];
    return cats.find((c) => c.extensions.includes(ext))?.id ?? 'general';
  }

  // ---------------- selection ----------------
  function clickRow(e: MouseEvent, d: Download) {
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (e.shiftKey && anchorId) {
      const a = rows.findIndex((r) => r.id === anchorId);
      const b = rows.findIndex((r) => r.id === d.id);
      if (a >= 0 && b >= 0) {
        if (!mod) selected.clear();
        for (let i = Math.min(a, b); i <= Math.max(a, b); i++) selected.add(rows[i].id);
      }
    } else if (mod) {
      if (selected.has(d.id)) selected.delete(d.id); else selected.add(d.id);
      anchorId = d.id;
    } else {
      selected.clear();
      selected.add(d.id);
      anchorId = d.id;
    }
    focusedId = d.id;
  }
  function moveFocus(delta: number, extend: boolean) {
    if (!rows.length) return;
    let i = rows.findIndex((r) => r.id === focused?.id);
    i = Math.max(0, Math.min(rows.length - 1, i + delta));
    const d = rows[i];
    if (!extend) selected.clear();
    selected.add(d.id);
    focusedId = d.id;
    if (!extend) anchorId = d.id;
    // keep it in view
    if (scroller) {
      const top = i * rowH, bottom = top + rowH + 40;
      if (top < scroller.scrollTop) scroller.scrollTop = top;
      else if (bottom > scroller.scrollTop + scroller.clientHeight) scroller.scrollTop = bottom - scroller.clientHeight;
    }
  }

  // ---------------- context menus ----------------
  function rowMenu(e: MouseEvent, d: Download) {
    e.preventDefault();
    if (!selected.has(d.id)) { selected.clear(); selected.add(d.id); anchorId = d.id; }
    focusedId = d.id;
    const list = targetIds(d);
    const multi = list.length > 1;
    const items: MenuItem[] = [
      { label: 'Open', icon: 'open-file', disabled: d.status !== 'complete' || multi, action: () => act(() => api.openFile(d.id)) },
      { label: 'Open folder', icon: 'open-folder', disabled: multi, action: () => act(() => api.openFolder(d.id)) },
      { label: 'Show progress window', icon: 'segments', disabled: d.status === 'complete' || multi, action: () => openProgress(d) },
      { sep: true },
      { label: 'Resume', icon: 'resume', disabled: !list.some((i) => ['paused', 'error', 'queued'].includes(byId(i)?.status ?? '')), action: () => resumeSel(list) },
      { label: 'Pause', icon: 'pause', disabled: !list.some((i) => { const x = byId(i); return !!x && (isActive(x) || x.status === 'queued'); }), action: () => pauseSel(list) },
      { label: 'Redownload', icon: 'redownload', disabled: multi, action: () => act(() => api.redownload(d.id)) },
      { label: 'Refresh link…', icon: 'refresh-link', disabled: multi || d.status === 'complete', action: () => (refreshModal = { id: d.id, url: d.url, busy: false, err: '' }) },
      { label: 'Copy link', icon: 'copy-link', disabled: multi, action: () => copyLink(d) },
      { sep: true },
      {
        label: 'Move to queue', icon: 'add-to-queue',
        sub: [
          ...store.queues.map((q) => ({ label: q.name, checked: !multi && d.queueId === q.id, action: () => act(() => api.moveToQueue(list, q.id)) })),
          { sep: true as const },
          { label: 'Remove from queue', icon: 'remove-from-queue' as IconName, action: () => act(() => api.moveToQueue(list, null)) },
        ],
      },
      {
        label: 'Category', icon: 'file-types',
        sub: (store.settings?.categories ?? []).map((c) => ({ label: c.name, checked: !multi && d.category === c.id, action: () => act(() => api.setCategory(list, c.id)) })),
      },
      ...(fileExt(d) ? [{
        label: `Always put .${fileExt(d)} files in`, icon: 'file-types' as IconName,
        sub: (store.settings?.categories ?? []).map((c) => ({
          label: c.name,
          checked: catForExt(fileExt(d)) === c.id,
          action: () => act(async () => {
            const ext = fileExt(d);
            await api.assignExtensions(c.id, [ext]);
            // re-file every download of that type that hasn't been put elsewhere by hand
            const same = store.downloads.filter((x) => fileExt(x) === ext && x.category !== c.id).map((x) => x.id);
            if (same.length) await api.setCategory(same, c.id);
            toast(`.${ext} files now go to ${c.name}`);
          }),
        })),
      }] : []),
      { label: 'Properties', icon: 'properties', disabled: multi, action: () => openProgress(d) },
      { sep: true },
      { label: multi ? `Delete ${list.length} downloads` : 'Delete', icon: 'delete', danger: true, action: () => askDelete(list) },
    ];
    menu = { x: e.clientX, y: e.clientY, items };
  }
  function headerMenu(e: MouseEvent) {
    e.preventDefault();
    menu = {
      x: e.clientX, y: e.clientY,
      items: [
        ...cols.map((c) => ({
          label: c.label, checked: c.visible, disabled: c.key === 'name',
          action: () => { c.visible = !c.visible; persistCols(); },
        })),
        { sep: true as const },
        { label: 'Reset columns', action: () => { cols = structuredClone(DEFAULT_COLS); persistCols(); } },
      ],
    };
  }
  function queueMenu(e: MouseEvent, qid: string) {
    e.preventDefault();
    const q = store.queues.find((x) => x.id === qid);
    if (!q) return;
    menu = {
      x: e.clientX, y: e.clientY,
      items: [
        { label: 'Start', icon: 'start-queue', disabled: q.running, action: () => act(() => api.startQueue(qid)) },
        { label: 'Stop', icon: 'stop-queue', disabled: !q.running, action: () => act(() => api.stopQueue(qid)) },
        { label: 'Edit schedule…', icon: 'scheduler', action: () => act(() => api.openWindow('scheduler', null, { queueId: qid })) },
        { sep: true },
        { label: 'Delete queue', icon: 'delete', danger: true, disabled: q.builtin, action: () => act(() => api.deleteQueue(qid)) },
      ],
    };
  }
  function categoryMenu(e: MouseEvent) {
    e.preventDefault();
    const open = () => act(() => api.openWindow('settings', null, { page: 'categories' }));
    menu = {
      x: e.clientX, y: e.clientY,
      items: [
        { label: 'Add category…', icon: 'plus', action: open },
        { label: 'Edit file types & folders…', icon: 'file-types', action: open },
        { label: 'Delete category…', icon: 'delete', action: open },
      ],
    };
  }
  function moreMenu(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    menu = {
      x: r.right - 230, y: r.bottom + 4,
      items: [
        { label: 'Add batch download…', icon: 'batch-download', action: () => addUrl({ batch: true }) },
        { label: 'Add from clipboard', icon: 'add-from-clipboard', action: () => addUrl({ fromClipboard: true }) },
        { label: 'Site grabber…', icon: 'site-grabber', action: () => act(() => api.openWindow('grabber')) },
        { sep: true },
        { label: 'Import link list…', icon: 'import', action: importList },
        { label: 'Export link list…', icon: 'export', action: exportList },
        { sep: true },
        { label: 'Settings…', icon: 'options', action: () => act(() => api.openWindow('settings')) },
        { label: 'About Open Download Manager', icon: 'help', action: () => act(() => api.openWindow('about')) },
        { label: 'Quit', icon: 'exit', action: () => api.quit() },
      ],
    };
  }

  // ---------------- column resize / reorder ----------------
  function startResize(e: PointerEvent, c: Col) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const x0 = e.clientX, w0 = colWidth(c);
    document.body.classList.add('resizing');
    const move = (ev: PointerEvent) => { c.width = Math.max(c.key === 'name' ? 120 : 56, Math.min(1600, w0 + ev.clientX - x0)); };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      document.body.classList.remove('resizing');
      persistCols();
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }
  let measureCtx: CanvasRenderingContext2D | null = null;
  function textWidth(t: string, font: string) {
    measureCtx ??= document.createElement('canvas').getContext('2d');
    if (!measureCtx) return t.length * 8;
    measureCtx.font = font;
    return measureCtx.measureText(t).width;
  }
  /** Double-click a column edge: fit the column to its widest content. */
  function autofit(c: Col) {
    const header = textWidth(c.label, "500 12px 'IBM Plex Sans'") + 28 + 18;
    let w = header;
    if (c.key === 'name') {
      for (const d of rows) w = Math.max(w, textWidth(d.fileName, "500 13px 'IBM Plex Sans'") + 34 + 10 + 28 + 6);
    } else {
      const idx = visibleCols.findIndex((x) => x.key === c.key);
      scroller?.querySelectorAll('tbody tr').forEach((tr) => {
        const td = (tr as HTMLTableRowElement).cells[idx];
        if (td && td.colSpan === 1) w = Math.max(w, td.scrollWidth + 2);
      });
    }
    c.width = Math.round(Math.min(1600, Math.max(56, w)));
    persistCols();
  }
  let dragCol: ColKey | null = null;
  function dropCol(target: ColKey) {
    if (!dragCol || dragCol === target) return;
    const from = cols.findIndex((c) => c.key === dragCol);
    const to = cols.findIndex((c) => c.key === target);
    const [c] = cols.splice(from, 1);
    cols.splice(to, 0, c);
    dragCol = null;
    persistCols();
  }
  function clickHeader(c: Col) {
    if (sort.key === c.key) sort.dir = -sort.dir;
    else sort = { key: c.key, dir: c.key === 'name' || c.key === 'queue' || c.key === 'status' ? 1 : -1 };
    persistCols();
  }

  // ---------------- drag rows to sidebar / drop links ----------------
  function rowDragStart(e: DragEvent, d: Download) {
    if (!selected.has(d.id)) { selected.clear(); selected.add(d.id); }
    e.dataTransfer!.setData('application/x-odm-ids', JSON.stringify(ids()));
    e.dataTransfer!.effectAllowed = 'move';
  }
  function sideDragOver(e: DragEvent, key: string) {
    if (e.dataTransfer?.types.includes('application/x-odm-ids')) {
      e.preventDefault();
      dragTarget = key;
    }
  }
  function sideDrop(e: DragEvent, kind: 'cat' | 'queue', id: string) {
    e.preventDefault();
    e.stopPropagation();
    dragTarget = null;
    const raw = e.dataTransfer?.getData('application/x-odm-ids');
    if (!raw) return;
    const list: string[] = JSON.parse(raw);
    if (kind === 'queue') act(() => api.moveToQueue(list, id));
    else act(() => api.setCategory(list, id));
  }
  function winDragOver(e: DragEvent) {
    const t = e.dataTransfer?.types ?? [];
    if (t.includes('application/x-odm-ids')) return;
    if (t.includes('text/uri-list') || t.includes('text/plain')) { e.preventDefault(); dropOver = true; }
  }
  function winDrop(e: DragEvent) {
    dropOver = false;
    const t = e.dataTransfer;
    if (!t || t.types.includes('application/x-odm-ids')) return;
    e.preventDefault();
    const text = t.getData('text/uri-list') || t.getData('text/plain');
    const urls = text.split(/\s+/).map((s) => s.trim()).filter((s) => /^https?:\/\//i.test(s));
    if (!urls.length) return;
    if (urls.length === 1) addUrl({ url: urls[0] });
    else act(() => api.openWindow('grabber', null, { links: urls.map((url) => ({ url })) }));
  }

  // ---------------- keyboard ----------------
  const FIELDS = 'input, textarea, select, [contenteditable]:not([contenteditable="false"])';
  const CONTROLS = 'button, a, [role=switch], [role=menuitem]';
  function onKey(e: KeyboardEvent) {
    const mod = isMac ? e.metaKey : e.ctrlKey;
    const target = e.target instanceof Element ? e.target : null;
    const inField = !!target?.closest(FIELDS);
    if (menu || confirmDelete || refreshModal || limitModal) return;
    // Enter / Space on a focused control activate that control, not the list
    if ((e.key === 'Enter' || e.key === ' ') && target?.closest(CONTROLS)) return;
    if (mod && e.key.toLowerCase() === 'n') { e.preventDefault(); addUrl(); return; }
    if (mod && e.key.toLowerCase() === 'k') { e.preventDefault(); searchEl?.focus(); searchEl?.select(); return; }
    if (mod && e.key === ',') { e.preventDefault(); act(() => api.openWindow('settings')); return; }
    if (inField) {
      if (e.key === 'Escape') { searchInput = ''; search = ''; (e.target as HTMLElement).blur(); }
      return;
    }
    if (mod && e.key.toLowerCase() === 'a') { e.preventDefault(); rows.forEach((r) => selected.add(r.id)); return; }
    if (e.key === 'ArrowDown') { e.preventDefault(); moveFocus(1, e.shiftKey); return; }
    if (e.key === 'ArrowUp') { e.preventDefault(); moveFocus(-1, e.shiftKey); return; }
    if (e.key === ' ' && focused) {
      e.preventDefault();
      const list = targetIds();
      if (list.some((i) => { const d = byId(i); return d && (isActive(d) || d.status === 'queued'); })) pauseSel(list);
      else resumeSel(list);
      return;
    }
    if ((e.key === 'Delete' || (isMac && e.key === 'Backspace' && e.metaKey)) && selected.size) { e.preventDefault(); askDelete(); return; }
    if (e.key === 'Enter' && focused) { e.preventDefault(); openItem(focused); }
  }

  let searchTimer: ReturnType<typeof setTimeout>;
  $effect(() => {
    const v = searchInput;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => (search = v), 120);
  });

  onMount(() => {
    initStore();
    api.kvGet('columns').then((raw) => {
      if (!raw) return;
      try {
        const saved = JSON.parse(raw);
        const known = new Map(DEFAULT_COLS.map((c) => [c.key, c]));
        const restored: Col[] = (saved.cols ?? []).filter((c: Col) => known.has(c.key)).map((c: Col) => ({ ...known.get(c.key)!, width: c.width, visible: c.visible }));
        for (const c of DEFAULT_COLS) if (!restored.find((r) => r.key === c.key)) restored.push(structuredClone(c));
        cols = restored;
        if (saved.sort) sort = saved.sort;
      } catch {}
    });
    // The backend re-emits the countdown every second: each event only
    // updates the one banner (never stacks), keyed by its token.
    const applyPower = (p: PowerCountdown | null) => {
      if (!p) { power = null; return; }
      const left = Math.max(0, Math.round(Number(p.remaining ?? p.seconds) || 0));
      const token = p.token ?? null;
      if (power && power.token === token && power.action === p.action) power.left = left;
      else power = { action: p.action, queue: p.queue, token, left };
    };
    const un = on<PowerCountdown | null>('power_countdown', applyPower);
    // a countdown that started before this window existed
    api.getPowerCountdown().then((p) => { if (p && !power) applyPower(p); }).catch(() => {});
    const t = setInterval(() => { if (power && power.left > 0) power.left--; }, 1000);
    const ro = new ResizeObserver(() => { if (scroller) { viewH = scroller.clientHeight; viewW = scroller.clientWidth; } });
    tick().then(() => scroller && ro.observe(scroller));
    return () => { un.then((u) => u()); clearInterval(t); ro.disconnect(); };
  });

  const libItems: { id: string; label: string; icon: IconName }[] = [
    { id: 'all', label: 'All downloads', icon: 'cat-all' },
    { id: 'active', label: 'Active', icon: 'cat-active' },
    { id: 'unfinished', label: 'Unfinished', icon: 'cat-unfinished' },
    { id: 'finished', label: 'Finished', icon: 'cat-finished' },
  ];
  const sel = (k: Filter['kind'], id: string) => filter.kind === k && filter.id === id;
  const summary = $derived.by(() => {
    const t = store.totals;
    if (!t) return '';
    const parts = [`${t.active} active`];
    if (t.paused) parts.push(`${t.paused} paused`);
    if (t.queued) parts.push(`${t.queued} queued`);
    if (t.scheduled) parts.push(`${t.scheduled} scheduled`);
    if (t.failed) parts.push(`${t.failed} failed`);
    return parts.join(' · ');
  });
  const powerLabel = (a: string) => (a === 'sleep' ? 'Sleeping' : a === 'shutdown' ? 'Shutting down' : 'Exiting');
</script>

<svelte:window onkeydown={onKey} ondragover={winDragOver} ondragleave={(e) => { if (!e.relatedTarget) dropOver = false; }} ondrop={winDrop} />

<div class="app" class:drop={dropOver}>
  <header class="head">
    <div class="brand">
      <span class="logo"><Icon name="odm-mark" size={19} stroke={2.6} /></span>
      <span>Open Download Manager</span>
    </div>
    <label class="search">
      <Icon name="search" size={14} />
      <span class="sr-only">Search downloads</span>
      <input bind:this={searchEl} type="search" placeholder="Search downloads, URLs, folders" bind:value={searchInput} spellcheck="false" />
      <span class="kbd mono">{modKey} K</span>
    </label>
    <div class="total mono">
      <span class="muted">Total</span>
      <span class="spd"><Icon name="download" size={13} stroke={2.4} />{f.speed(store.totals?.speedBps)}</span>
    </div>
  </header>

  {#if power}
    <div class="power" role="alert">
      <Icon name={power.action === 'sleep' ? 'sleep' : power.action === 'shutdown' ? 'shutdown' : 'exit'} size={16} />
      <span>“{power.queue}” finished. {powerLabel(power.action)} in <b class="mono">{power.left}s</b>.</span>
      <span class="spacer"></span>
      <button class="btn sm" onclick={() => act(() => api.cancelPowerAction())}>Cancel</button>
    </div>
  {/if}

  <div class="body">
    <aside class="side">
      <nav aria-label="Library">
        <div class="label">Library</div>
        {#each libItems as it}
          <button class="nav" class:on={sel('lib', it.id)} onclick={() => (filter = { kind: 'lib', id: it.id })}>
            <Icon name={it.icon} size={16} /><span>{it.label}</span><span class="cnt mono">{counts[it.id] ?? 0}</span>
          </button>
        {/each}
      </nav>
      <nav aria-label="Categories">
        <div class="label">Categories</div>
        {#each store.settings?.categories ?? [] as c (c.id)}
          <button class="nav" class:on={sel('cat', c.id)} class:dropt={dragTarget === 'cat:' + c.id}
            onclick={() => (filter = { kind: 'cat', id: c.id })} oncontextmenu={categoryMenu}
            ondragover={(e) => sideDragOver(e, 'cat:' + c.id)} ondragleave={() => (dragTarget = null)} ondrop={(e) => sideDrop(e, 'cat', c.id)}>
            <Icon name={(c.icon in ICONS ? c.icon : 'cat-general') as IconName} size={16} /><span>{c.name}</span><span class="cnt mono">{counts['cat:' + c.id] ?? 0}</span>
          </button>
        {/each}
      </nav>
      <nav aria-label="Queues">
        <div class="label">Queues</div>
        {#each store.queues as q (q.id)}
          <button class="nav" class:on={sel('queue', q.id)} class:dropt={dragTarget === 'q:' + q.id}
            onclick={() => (filter = { kind: 'queue', id: q.id })} oncontextmenu={(e) => queueMenu(e, q.id)}
            ondragover={(e) => sideDragOver(e, 'q:' + q.id)} ondragleave={() => (dragTarget = null)} ondrop={(e) => sideDrop(e, 'queue', q.id)}>
            <Icon name={q.builtin ? 'q-main' : 'st-scheduled'} size={16} />
            <span>{q.name}</span>
            {#if q.running}<span class="run" title="Running"></span>{/if}
            <span class="cnt mono">{counts['q:' + q.id] ?? 0}</span>
          </button>
        {/each}
      </nav>
    </aside>

    <main class="main">
      <div role="toolbar" aria-label="Download actions" class="toolbar">
        <button class="btn tb primary" onclick={() => addUrl()} title="Add URL ({modKey}N)"><Icon name="add-url" size={16} stroke={2.2} />Add URL</button>
        <span class="div"></span>
        <button class="btn tb" disabled={!canResume} onclick={() => resumeSel()}><Icon name="resume" size={16} />Resume</button>
        <button class="btn tb" disabled={!canPause} onclick={() => pauseSel()}><Icon name="pause" size={16} />Pause</button>
        <button class="btn tb" disabled={!store.totals?.active && !store.totals?.queued} onclick={() => act(() => api.pauseAll())}><Icon name="pause-all" size={16} />Pause all</button>
        <button class="btn tb" disabled={!selected.size} onclick={() => askDelete()}><Icon name="delete" size={16} />Delete</button>
        <button class="btn tb" disabled={!store.totals?.complete} onclick={() => act(async () => { const n = await api.deleteCompleted(); toast(`Removed ${n} completed`); })}><Icon name="delete-completed" size={16} />Delete completed</button>
        <span class="div"></span>
        <button class="btn tb" onclick={() => act(() => api.startQueue(currentQueueId()))}><Icon name="start-queue" size={16} />Start queue</button>
        <button class="btn tb" onclick={() => act(() => api.stopQueue(currentQueueId()))}><Icon name="stop-queue" size={16} />Stop queue</button>
        <button class="btn tb" onclick={() => act(() => api.openWindow('scheduler', null, filter.kind === 'queue' ? { queueId: filter.id } : null))}><Icon name="scheduler" size={16} />Scheduler</button>
        <button class="btn tb" onclick={() => act(() => api.openWindow('grabber'))}><Icon name="site-grabber" size={16} />Grabber</button>
        <button class="btn tb" class:limit={limitOn} aria-pressed={limitOn} onclick={toggleLimit}
          oncontextmenu={(e) => { e.preventDefault(); limitModal = { value: String(limitMB || 25) }; }}
          title="Click to toggle · right-click to set the limit">
          <Icon name="speed-limit" size={16} />{limitOn ? `Limit ${limitMB} MB/s` : 'No limit'}
        </button>
        <span class="push"></span>
        <button class="btn tb icon" aria-label="More" onclick={moreMenu}><Icon name="more" size={16} /></button>
        <button class="btn tb icon" aria-label="Options" title="Settings ({modKey},)" onclick={() => act(() => api.openWindow('settings'))}><Icon name="options" size={16} /></button>
      </div>

      <div class="tablewrap" bind:this={scroller} onscroll={() => (scrollTop = scroller!.scrollTop)}>
        <table style:width="{tableWidth}px">
          <colgroup>{#each visibleCols as c (c.key)}<col style:width="{colWidth(c)}px" />{/each}</colgroup>
          <thead oncontextmenu={headerMenu}>
            <tr>
              {#each visibleCols as c (c.key)}
                <th class:r={c.align === 'right'} aria-sort={sort.key === c.key ? (sort.dir > 0 ? 'ascending' : 'descending') : 'none'}
                  ondragover={(e) => { if (dragCol) e.preventDefault(); }} ondrop={() => dropCol(c.key)}>
                  <button class="th" draggable="true" onclick={() => clickHeader(c)}
                    ondragstart={(e) => { dragCol = c.key; e.dataTransfer!.setData('application/x-odm-col', c.key); }}
                    ondragend={() => (dragCol = null)}>
                    {c.label}
                    {#if sort.key === c.key}<Icon name={sort.dir > 0 ? 'sort-asc' : 'sort-desc'} size={12} />{/if}
                  </button>
                  <span class="grip" role="separator" aria-orientation="vertical" aria-label="Resize {c.label} column"
                    title="Drag to resize · double-click to fit" onpointerdown={(e) => startResize(e, c)} ondblclick={() => autofit(c)}></span>
                </th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#if virtual && vStart > 0}<tr style:height="{vStart * rowH}px"><td colspan={visibleCols.length}></td></tr>{/if}
            {#each shown as d (d.id)}
              <tr class:sel={selected.has(d.id)} style:height="{rowH}px" draggable="true"
                onclick={(e) => clickRow(e, d)} ondblclick={() => openItem(d)} oncontextmenu={(e) => rowMenu(e, d)}
                ondragstart={(e) => rowDragStart(e, d)}>
                {#each visibleCols as c (c.key)}
                  {#if c.key === 'name'}
                    <td><div class="name"><FileBadge ext={d.ext} /><span class="ellipsis fname" title={d.fileName}>{d.fileName}</span></div></td>
                  {:else if c.key === 'size'}
                    <td class="mono">{f.bytes(d.totalBytes)}</td>
                  {:else if c.key === 'progress'}
                    <td>
                      <div class="prog">
                        <div class="track" class:ind={d.status === 'connecting' && !d.receivedBytes}><div class="fill" style:width="{f.pct(d)}%" style:background={f.statusColor(d.status)}></div></div>
                        <span class="mono pct">{f.pct(d)}%</span>
                      </div>
                    </td>
                  {:else if c.key === 'speed'}
                    <td class="mono r">{f.speed(d.speedBps)}</td>
                  {:else if c.key === 'eta'}
                    <td class="mono r">{d.status === 'downloading' ? f.eta(d.etaSecs) : '—'}</td>
                  {:else if c.key === 'status'}
                    <td><StatusPill {d} /></td>
                  {:else if c.key === 'queue'}
                    <td class="small muted ellipsis">{queueName(d.queueId)}</td>
                  {:else}
                    <td class="small muted">{f.added(d.addedAt)}</td>
                  {/if}
                {/each}
              </tr>
            {/each}
            {#if virtual && vEnd < rows.length}<tr style:height="{(rows.length - vEnd) * rowH}px"><td colspan={visibleCols.length}></td></tr>{/if}
          </tbody>
        </table>
        {#if store.ready && !rows.length}
          <div class="empty">
            <Icon name={search ? 'search' : 'download'} size={40} stroke={1.75} />
            <div class="muted">{search ? 'No downloads match your search' : 'No downloads here'}</div>
            {#if !search}<button class="btn" onclick={() => addUrl()}><Icon name="add-url" size={16} />Add URL</button>{/if}
            {#if !search && !store.downloads.length}<div class="muted small">Tip: drag a link from your browser onto this window.</div>{/if}
          </div>
        {/if}
      </div>

      {#if focused}
        {@const d = focused}
        <section class="details" aria-label="Selected download">
          <div class="dtop">
            <FileBadge ext={d.ext} large />
            <div class="dname">
              <div class="ellipsis big selectable">{d.fileName}</div>
              <div class="ellipsis mono small muted selectable" title={d.url}>{d.url}</div>
            </div>
            <StatusPill {d} />
            {#if isActive(d) || d.status === 'queued'}
              <button class="btn tb" onclick={() => pauseSel([d.id])}><Icon name="pause" size={15} />Pause</button>
            {:else if d.status === 'complete'}
              <button class="btn tb" onclick={() => act(() => api.openFile(d.id))}><Icon name="open-file" size={15} />Open file</button>
            {:else if d.status === 'error'}
              <button class="btn tb" onclick={() => (refreshModal = { id: d.id, url: d.url, busy: false, err: '' })}><Icon name="refresh-link" size={15} />Refresh link</button>
              <button class="btn tb" onclick={() => resumeSel([d.id])}><Icon name="resume" size={15} />Retry</button>
            {:else}
              <button class="btn tb" onclick={() => resumeSel([d.id])}><Icon name="resume" size={15} />Resume</button>
            {/if}
            <button class="btn tb" onclick={() => act(() => api.openFolder(d.id))}><Icon name="open-folder" size={15} />Open folder</button>
            <button class="btn tb" onclick={() => copyLink(d)}><Icon name="copy-link" size={15} />Copy link</button>
          </div>
          {#if d.status === 'error' && d.error}<div class="errbox">{d.error}</div>{/if}
          <div class="dbody">
            <dl>
              <dt>Downloaded</dt><dd class="mono">{f.bytes(d.receivedBytes)} of {f.bytes(d.totalBytes)}</dd>
              <dt>Speed</dt><dd class="mono">{f.speed(d.speedBps)}</dd>
              <dt>Time left</dt><dd class="mono">{d.status === 'downloading' ? f.eta(d.etaSecs) : '—'}</dd>
              <dt>Connections</dt><dd class="mono">{d.activeConnections} active / {d.maxConnections} max</dd>
              <dt>Resume</dt><dd>{d.resumable ? 'Supported (byte ranges)' : 'Not supported by server'}</dd>
              <dt>Save to</dt><dd class="mono ellipsis selectable" title={d.saveDir}>{d.saveDir}</dd>
            </dl>
            <div class="viz">
              <div>
                <div class="cap"><span>Segments</span><span class="mono">{f.pct(d)}%</span></div>
                <SegmentMap segments={d.segments} status={d.status} total={d.totalBytes} />
              </div>
              <Sparkline samples={d.speedHistory} />
            </div>
          </div>
        </section>
      {/if}
    </main>
  </div>

  <footer class="status">
    <span>{summary}</span>
    <span class="mono">↓ {f.speed(store.totals?.speedBps)}</span>
    <span>Speed limit: {limitOn ? `${limitMB} MB/s total` : 'off'}</span>
    <span class="ext" title={store.ext.bridgeError ?? undefined}><span class="dot" class:ok={store.ext.connected && !store.ext.bridgeError} class:bad={!!store.ext.bridgeError}></span>{store.ext.bridgeError ? 'Browser bridge offline: port in use' : store.ext.connected ? `Browser extension connected${store.ext.browsers.length ? ` (${store.ext.browsers.join(', ')})` : ''}` : 'Browser extension not detected'}</span>
    <span class="spacer"></span>
    <span class="mono">{store.totals?.freeBytes != null ? `${f.bytes(store.totals.freeBytes)} free` : ''}</span>
  </footer>
</div>

{#if menu}<ContextMenu x={menu.x} y={menu.y} items={menu.items} onclose={() => (menu = null)} />{/if}

{#if confirmDelete}
  <Modal title={confirmDelete.ids.length === 1 ? 'Delete 1 download?' : `Delete ${confirmDelete.ids.length} downloads?`} onclose={() => (confirmDelete = null)}>
    <p class="muted" style="margin:0">Unfinished parts are always removed. Finished files stay on disk unless you choose otherwise.</p>
    <label class="check"><input type="checkbox" bind:checked={confirmDelete.files} />Also delete files from disk</label>
    {#snippet footer()}
      <button class="btn" onclick={() => (confirmDelete = null)}>Cancel</button>
      <button class="btn primary" onclick={doDelete}>Delete</button>
    {/snippet}
  </Modal>
{/if}

{#if refreshModal}
  <Modal title="Refresh download link" width={520} onclose={() => (refreshModal = null)}>
    <p class="muted" style="margin:0">Paste a fresh address for the same file. Progress is kept when the server reports the same size.</p>
    <input class="input mono" bind:value={refreshModal.url} spellcheck="false" onkeydown={(e) => e.key === 'Enter' && doRefresh()} />
    {#if refreshModal.err}<div class="errbox">{refreshModal.err}</div>{/if}
    {#snippet footer()}
      <button class="btn" onclick={() => (refreshModal = null)}>Cancel</button>
      <button class="btn primary" disabled={refreshModal!.busy} onclick={doRefresh}>{refreshModal!.busy ? 'Checking…' : 'Update link'}</button>
    {/snippet}
  </Modal>
{/if}

{#if limitModal}
  <Modal title="Global speed limit" width={340} onclose={() => (limitModal = null)}>
    <label class="field"><span>Combined limit for all downloads</span>
      <div style="display:flex;gap:8px;align-items:center">
        <input class="input mono" style="width:120px;text-align:right" bind:value={limitModal.value} inputmode="decimal" onkeydown={(e) => e.key === 'Enter' && saveLimit()} />
        <span class="mono muted">MB/s</span>
      </div>
    </label>
    {#snippet footer()}
      <button class="btn" onclick={() => (limitModal = null)}>Cancel</button>
      <button class="btn primary" onclick={saveLimit}>Turn on limit</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .app { height: 100%; display: flex; flex-direction: column; background: var(--bg); }
  .app.drop { outline: 2px dashed var(--accent); outline-offset: -6px; }
  .head { display: flex; align-items: center; gap: 16px; padding: 10px 16px; background: var(--surface); border-bottom: 1px solid var(--border); flex: none; }
  .brand { display: flex; align-items: center; gap: 10px; font-weight: 600; font-size: var(--fs-lg); white-space: nowrap; }
  .logo { width: 26px; height: 26px; border-radius: 7px; background: var(--accent); color: var(--on-accent); display: flex; align-items: center; justify-content: center; }
  .search { flex: 1 1 280px; max-width: 460px; display: flex; align-items: center; gap: 8px; padding: 0 12px; height: 34px; border-radius: var(--r-md); background: var(--surface-2); border: 1px solid var(--border); color: var(--text-muted); }
  .search:focus-within { border-color: var(--accent); }
  .search input { flex: 1; min-width: 0; background: transparent; border: 0; outline: none; color: var(--text); }
  .search input::-webkit-search-cancel-button { filter: grayscale(1); }
  .kbd { font-size: 11px; padding: 1px 6px; border-radius: 4px; border: 1px solid var(--border); white-space: nowrap; }
  .total { margin-left: auto; display: flex; align-items: center; gap: 14px; font-size: var(--fs-sm); white-space: nowrap; }
  .spd { display: flex; align-items: center; gap: 6px; color: var(--accent); font-weight: 500; }

  .power { display: flex; align-items: center; gap: 10px; padding: 8px 16px; background: var(--danger-soft); color: var(--text); border-bottom: 1px solid var(--danger); }

  .body { flex: 1; min-height: 0; display: flex; }
  .side { width: var(--sidebar-w); flex: none; background: var(--surface); border-right: 1px solid var(--border); padding: 14px 10px; display: flex; flex-direction: column; gap: 18px; overflow-y: auto; }
  .side nav { display: flex; flex-direction: column; gap: 2px; }
  .side .label { padding: 0 10px 6px; }
  .nav { display: flex; align-items: center; gap: 9px; width: 100%; padding: 7px 10px; border-radius: 7px; border: 0; background: transparent; color: var(--text-muted); text-align: left; transition: background var(--motion); }
  .nav span:nth-of-type(1) { flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .nav:hover { background: var(--surface-2); color: var(--text); }
  .nav.on { background: var(--accent-soft); color: var(--text); font-weight: 600; }
  .nav.on :global(svg) { color: var(--accent); }
  .nav.dropt { outline: 1px solid var(--accent); }
  .cnt { font-size: 11px; color: var(--text-muted); font-weight: 400; }
  .run { width: 7px; height: 7px; border-radius: 50%; background: var(--success); flex: none; }

  .main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 12px; padding: 12px 16px 16px; }
  .toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; flex: none; }
  .toolbar .div { width: 1px; height: 22px; background: var(--border); margin: 0 4px; }
  .toolbar .push { margin-left: auto; }
  .btn.limit { border-color: var(--warning); color: var(--warning); }

  .tablewrap { flex: 1; min-height: 120px; overflow: auto; border: 1px solid var(--border); border-radius: var(--r-lg); background: var(--surface); position: relative; }
  table { border-collapse: separate; border-spacing: 0; table-layout: fixed; font-size: var(--fs-md); }
  thead th { position: sticky; top: 0; z-index: 2; background: var(--surface); text-align: left; padding: 0; border-bottom: 1px solid var(--border); color: var(--text-muted); font-size: var(--fs-sm); font-weight: 500; }
  th .th { display: flex; align-items: center; gap: 4px; width: 100%; height: 36px; padding: 0 14px; border: 0; background: transparent; color: inherit; font-weight: 500; text-align: left; }
  th.r .th { justify-content: flex-end; }
  th .grip { position: absolute; right: 0; top: 0; bottom: 0; width: 8px; cursor: col-resize; z-index: 3; touch-action: none; }
  th .grip::after { content: ''; position: absolute; left: 7px; top: 9px; bottom: 9px; width: 1px; background: var(--border-subtle); }
  th .grip:hover::after { background: var(--accent); width: 2px; left: 6px; }
  :global(body.resizing), :global(body.resizing *) { cursor: col-resize !important; }

  th { position: relative; }
  td { padding: 0 14px; border-bottom: 1px solid var(--border-subtle); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  :global([data-density='compact']) td { padding: 0 14px; }
  td.r { text-align: right; }
  .small { font-size: var(--fs-sm); }
  tbody tr { transition: background var(--motion); }
  tbody tr:hover { background: var(--surface-2); }
  tbody tr.sel { background: var(--row-selected); box-shadow: inset 3px 0 0 var(--accent); }
  .name { display: flex; align-items: center; gap: 10px; min-width: 0; }
  .fname { font-weight: 500; min-width: 0; }
  .prog { display: flex; align-items: center; gap: 10px; }
  .track { flex: 1; max-width: 120px; height: var(--progress-h); border-radius: 3px; background: var(--track); overflow: hidden; position: relative; }
  .fill { height: 100%; }
  .track.ind::after { content: ''; position: absolute; inset: 0; background: repeating-linear-gradient(-45deg, transparent 0 6px, var(--accent-soft) 6px 12px); animation: stripe .8s linear infinite; }
  @keyframes stripe { from { background-position: 0 0; } to { background-position: 17px 0; } }
  .pct { font-size: var(--fs-sm); width: 38px; text-align: right; }
  .empty { position: absolute; inset: 37px 0 0 0; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 12px; color: var(--text-muted); pointer-events: none; }
  .empty button { pointer-events: auto; }

  .details { flex: none; border: 1px solid var(--border); border-radius: var(--r-lg); background: var(--surface); padding: 16px; display: flex; flex-direction: column; gap: 14px; }
  .dtop { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
  .dname { flex: 1 1 260px; min-width: 0; }
  .big { font-weight: 600; font-size: var(--fs-xl); }
  .dbody { display: flex; flex-wrap: wrap; gap: 24px; }
  dl { flex: 1 1 260px; margin: 0; display: grid; grid-template-columns: 110px minmax(0, 1fr); row-gap: 7px; column-gap: 12px; font-size: var(--fs-sm); }
  dt { color: var(--text-muted); }
  dd { margin: 0; min-width: 0; }
  .viz { flex: 2 1 380px; min-width: 0; display: flex; flex-direction: column; gap: 12px; }
  .cap { display: flex; justify-content: space-between; font-size: var(--fs-sm); color: var(--text-muted); margin-bottom: 6px; }

  .status { flex: none; display: flex; flex-wrap: wrap; gap: 6px 20px; align-items: center; padding: 7px 16px; background: var(--surface); border-top: 1px solid var(--border); color: var(--text-muted); font-size: var(--fs-sm); }
  .ext { display: flex; align-items: center; gap: 6px; }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--text-muted); opacity: .6; }
  .dot.ok { background: var(--success); opacity: 1; }
  .dot.bad { background: var(--danger); opacity: 1; }

  @media (max-height: 760px) { .details .dbody { display: none; } }
  @media (max-width: 1000px) { .side { width: 180px; } }
</style>
