// Live download list shared by a window, updated from engine events.
import { api, on, type Download, type ProgressItem, type Queue, type Settings, type Totals } from './ipc';

export const store = $state({
  downloads: [] as Download[],
  queues: [] as Queue[],
  settings: null as Settings | null,
  totals: null as Totals | null,
  ext: { connected: false, browsers: [] as string[] },
  ready: false,
});

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
    // keep speed history continuity if the full record lags
    store.downloads[i] = d;
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
  await Promise.all([
    on<ProgressItem[]>('progress_batch', (items) => {
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
    }),
    on<Download>('download_changed', upsert),
    on<Download>('download_added', upsert),
    on<string>('download_removed', (id) => {
      const i = index.get(id);
      if (i != null) {
        store.downloads.splice(i, 1);
        reindex();
      }
    }),
    on<Totals>('totals', (t) => {
      store.totals = t;
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
    on<Queue[]>('queues_changed', (q) => (store.queues = q)),
    on<Settings>('settings_changed', (s) => (store.settings = s)),
    on<{ connected: boolean; browsers: string[] }>('extension_status', (e) => (store.ext = e)),
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
  store.queues = queues;
  store.settings = settings;
  store.totals = totals;
  store.ext = { connected: ext.connected, browsers: ext.browsers };
  store.ready = true;
  void opts;
}
