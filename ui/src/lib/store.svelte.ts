// Live download list shared by a window, updated from engine events.
import { SvelteSet } from 'svelte/reactivity';
import { api, on, type Download, type ProgressItem, type Queue, type Settings, type Totals } from './ipc';

export const store = $state({
  downloads: [] as Download[],
  queues: [] as Queue[],
  settings: null as Settings | null,
  totals: null as Totals | null,
  ext: { connected: false, browsers: [] as string[], bridgeError: null as string | null },
  ready: false,
});

/** Selected download ids (main window). Removed downloads leave the selection. */
export const selection = new SvelteSet<string>();

const index = new Map<string, number>();

function reindex() {
  index.clear();
  store.downloads.forEach((d, i) => index.set(d.id, i));
}

function upsert(d: Download) {
  const i = index.get(d.id);
  if (i == null) {
    store.downloads.push(d);
    index.set(d.id, store.downloads.length - 1);
  } else {
    // keep the locally accumulated speed history if the record has none
    if (!d.speedHistory?.length) d.speedHistory = store.downloads[i].speedHistory;
    store.downloads[i] = d;
  }
}

function remove(id: string) {
  selection.delete(id);
  const i = index.get(id);
  if (i != null) {
    store.downloads.splice(i, 1);
    reindex();
  }
}

function progress(items: ProgressItem[]) {
  for (const p of items) {
    const i = index.get(p.id);
    if (i == null) continue;
    const d = store.downloads[i];
    d.receivedBytes = p.receivedBytes;
    if (p.totalBytes != null) d.totalBytes = p.totalBytes;
    d.speedBps = p.speedBps;
    d.etaSecs = p.etaSecs;
    d.activeConnections = p.activeConnections;
    d.segments = p.segments;
    d.status = p.status;
  }
}

export function byId(id: string): Download | undefined {
  const i = index.get(id);
  return i == null ? undefined : store.downloads[i];
}

let started = false;

export async function initStore(opts: { queues?: boolean; settings?: boolean } = {}) {
  if (started) return;
  started = true;

  // Download events that arrive while the initial list is loading are
  // buffered and replayed on top of it, so the list reply (a snapshot taken
  // when the request ran) never overwrites newer state.
  let loaded = false;
  const pending: (() => void)[] = [];
  const apply = (fn: () => void) => (loaded ? fn() : pending.push(fn));
  let queuesEvent = false;
  let settingsEvent = false;

  await Promise.all([
    on<ProgressItem[]>('progress_batch', (items) => apply(() => progress(items))),
    on<Download>('download_changed', (d) => apply(() => upsert(d))),
    on<Download>('download_added', (d) => apply(() => upsert(d))),
    on<string>('download_removed', (id) => apply(() => remove(id))),
    on<Totals>('totals', (t) => {
      store.totals = t;
      if (!loaded) return;
      // one sparkline sample per second for active downloads
      for (const d of store.downloads) {
        if (d.status === 'downloading' || d.status === 'connecting') {
          const h = d.speedHistory ?? [];
          h.push(d.speedBps);
          if (h.length > 60) h.splice(0, h.length - 60);
          d.speedHistory = h;
        }
      }
    }),
    on<Queue[]>('queues_changed', (q) => { queuesEvent = true; store.queues = q; }),
    on<Settings>('settings_changed', (s) => { settingsEvent = true; store.settings = s; }),
    on<{ connected: boolean; browsers: string[] }>('extension_status', (e) => (store.ext = { ...store.ext, ...e })),
    on<{ port: number; error: string } | null>('bridge_error', (e) => (store.ext = { ...store.ext, bridgeError: e?.error ?? null })),
  ]);
  const [list, queues, settings, totals, ext] = await Promise.all([
    api.listDownloads(),
    api.listQueues(),
    api.getSettings(),
    api.totals(),
    api.extensionStatus(),
  ]);
  store.downloads = list;
  reindex();
  for (const fn of pending) fn();
  pending.length = 0;
  loaded = true;
  if (!queuesEvent) store.queues = queues;
  if (!settingsEvent) store.settings = settings;
  if (!store.totals) store.totals = totals;
  store.ext = { connected: ext.connected, browsers: ext.browsers, bridgeError: ext.bridgeError ?? null };
  store.ready = true;
  void opts;
}
