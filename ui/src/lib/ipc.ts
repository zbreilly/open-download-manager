import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type DownloadStatus = 'queued' | 'connecting' | 'downloading' | 'paused' | 'assembling' | 'complete' | 'error';
export type SegState = 'idle' | 'connecting' | 'receiving' | 'reconnecting' | 'done';

export interface Segment { start: number; end: number; received: number; state: SegState; speedBps: number }

export interface Download {
  id: string;
  url: string;
  finalUrl: string | null;
  fileName: string;
  ext: string;
  category: string;
  queueId: string | null;
  saveDir: string;
  totalBytes: number | null;
  receivedBytes: number;
  speedBps: number;
  etaSecs: number | null;
  status: DownloadStatus;
  scheduledAt: string | null;
  resumable: boolean;
  maxConnections: number;
  activeConnections: number;
  segments: Segment[];
  speedHistory: number[];
  error: string | null;
  addedAt: string;
  completedAt: string | null;
  mime: string | null;
  protocol: string | null;
  referrer: string | null;
  checksum: { algo: string; expected: string } | null;
  checksumOk: boolean | null;
  speedLimitBps: number | null;
  whenFinished: string;
  manual: boolean;
  description: string | null;
  /** an HLS/DASH stream (the URL is its manifest) */
  stream?: StreamChoice | null;
  /** a note for the finished download (e.g. audio saved as a separate file) */
  notice?: string | null;
  /** files written beside the main one */
  extraFiles?: string[];
}

export interface ProgressItem {
  id: string;
  receivedBytes: number;
  totalBytes: number | null;
  speedBps: number;
  etaSecs: number | null;
  activeConnections: number;
  segments: Segment[];
  status: DownloadStatus;
}

export interface Totals {
  speedBps: number; active: number; paused: number; queued: number; scheduled: number;
  failed: number; complete: number; freeBytes: number | null; limitOn: boolean; limitBps: number;
}

export interface Queue {
  id: string; name: string; builtin: boolean; maxConcurrent: number; retryFailed: number;
  startEnabled: boolean; startTime: string; stopEnabled: boolean; stopTime: string;
  days: boolean[]; onFinish: string; order: string[]; running: boolean; nextRun: string | null;
}

/** A queue as sent to save_queue: settings fields, `order` only when reordering. */
export type QueueSettings = Omit<Queue, 'order'> & { order?: string[] };

/** power_countdown event / get_power_countdown payload. `seconds` is the time left. */
export interface PowerCountdown { action: string; seconds: number; queue: string; token?: string; remaining?: number }

export interface Category { id: string; name: string; icon: string; extensions: string[]; folder: string | null }

export interface Settings {
  launchAtLogin: boolean; minimizeToTray: boolean; clipboardMonitor: boolean; showProgressOnStart: boolean;
  showCompleteDialog: boolean; confirmDelete: boolean;
  defaultConnections: number; maxSimultaneous: number; globalLimitEnabled: boolean; globalLimitBps: number;
  perDownloadLimitEnabled: boolean; perDownloadLimitBps: number; limitOnStartup: boolean;
  captureEnabled: boolean; captureExtensions: string[]; bypassKey: string; capturePausedUntil: number | null;
  captureMinSizeKb: number; extensionPort: number;
  categories: Category[]; defaultFolder: string; tempFolder: string | null;
  proxyMode: string; proxyHost: string; proxyPort: number; proxyUser: string; proxyPass: string;
  notifyComplete: boolean; notifyFailure: boolean; notifyQueueFinished: boolean;
  theme: string; density: string; traySpeed: boolean;
  /** Main toolbar buttons: 'hover' (icons that open on hover), 'always' (full size) or 'icons' (always collapsed). */
  toolbarLabels: 'hover' | 'always' | 'icons';
  /** Window title bars: 'custom' (drawn by the app) or 'native' (the operating system's). */
  titleBar: 'custom' | 'native';
  userAgent: string; maxRetries: number; retryDelaySecs: number; timeoutSecs: number; minSegmentKb: number; verifyTls: boolean;
  allowInsecureRedirects: boolean;
}

export interface ProbeResult {
  finalUrl: string; fileName: string; totalBytes: number | null; mime: string | null;
  resumable: boolean; protocol: string; category: string;
  /** the link is an HLS/DASH manifest */
  stream?: StreamInfo;
}

export interface StreamVariant {
  id: string; label: string; width: number | null; height: number | null; bandwidth: number | null;
  codecs: string | null; estBytes: number | null; separateAudio: boolean; ext: string;
}
export interface StreamAudio { id: string; label: string; language: string | null; bandwidth: number | null; default: boolean }
export interface StreamInfo {
  kind: 'hls' | 'dash'; protected: boolean; live: boolean; blocked?: string;
  durationSecs: number | null; variants: StreamVariant[]; audio: StreamAudio[];
}
export interface StreamChoice { kind: string; variant: string | null; audio: string | null }

export interface AddRequest {
  url: string;
  fileName?: string | null;
  saveDir?: string | null;
  category?: string | null;
  queueId?: string | null;
  start: boolean;
  connections?: number;
  referrer?: string | null;
  cookies?: string | null;
  userAgent?: string | null;
  headers?: [string, string][];
  username?: string | null;
  password?: string | null;
  checksum?: { algo: string; expected: string } | null;
  onExists?: string | null;
  rememberFolder?: boolean;
  totalBytes?: number | null;
  resumable?: boolean | null;
  mime?: string | null;
  stream?: StreamChoice | null;
}

export interface GrabbedLink { url: string; fileName: string; ext: string; category: string; source: string }

export const api = {
  listDownloads: () => invoke<Download[]>('list_downloads'),
  getDownload: (id: string) => invoke<Download | null>('get_download', { id }),
  probe: (url: string, opts?: Record<string, unknown>) => invoke<ProbeResult>('probe_url', { url, opts }),
  add: (req: AddRequest) => invoke<string>('add_download', { req }),
  addMany: (reqs: AddRequest[]) => invoke<string[]>('add_many', { reqs }),
  pause: (ids: string[]) => invoke<void>('pause', { ids }),
  pauseAll: () => invoke<void>('pause_all'),
  resume: (ids: string[]) => invoke<void>('resume', { ids }),
  delete: (ids: string[], deleteFiles: boolean) => invoke<void>('delete', { ids, deleteFiles }),
  deleteCompleted: () => invoke<number>('delete_completed'),
  redownload: (id: string) => invoke<void>('redownload', { id }),
  refreshLink: (id: string, newUrl: string) => invoke<void>('refresh_link', { id, newUrl }),
  setGlobalLimit: (bps: number | null) => invoke<void>('set_global_limit', { bps }),
  setDownloadLimit: (id: string, bps: number | null) => invoke<void>('set_download_limit', { id, bps }),
  updateDownload: (id: string, patch: Record<string, unknown>) => invoke<void>('update_download', { id, patch }),
  setCategory: (ids: string[], category: string) => invoke<void>('set_category', { ids, category }),
  moveToQueue: (ids: string[], queueId: string | null) => invoke<void>('move_to_queue', { ids, queueId }),
  openFile: (id: string) => invoke<void>('open_file', { id }),
  openFolder: (id: string) => invoke<void>('open_folder', { id }),
  openPath: (path: string) => invoke<void>('open_path', { path }),
  listQueues: () => invoke<Queue[]>('list_queues'),
  /** `order` is optional: without it the engine keeps the queue's current order. */
  saveQueue: (queue: QueueSettings) => invoke<Queue>('save_queue', { queue }),
  deleteQueue: (id: string) => invoke<void>('delete_queue', { id }),
  startQueue: (id: string) => invoke<void>('start_queue', { id }),
  stopQueue: (id: string) => invoke<void>('stop_queue', { id }),
  reorderQueue: (id: string, order: string[]) => invoke<void>('reorder_queue', { id, order }),
  getSettings: () => invoke<Settings>('get_settings'),
  defaultSettings: () => invoke<Settings>('default_settings'),
  saveSettings: (settings: Settings) => invoke<void>('save_settings', { settings }),
  recentFolders: (limit = 4) => invoke<string[]>('recent_folders', { limit }),
  totals: () => invoke<Totals>('totals'),
  targetExists: (dir: string, name: string) => invoke<boolean>('target_exists', { dir, name }),
  folderFor: (category: string) => invoke<string>('folder_for', { category }),
  expandBatch: (pattern: string) => invoke<string[]>('expand_batch', { pattern }),
  grab: (url: string, depth: number, extensions: string[], sameHost = true) =>
    invoke<GrabbedLink[]>('grab', { url, depth, extensions, sameHost }),
  openWindow: (view: string, key?: string | null, params?: unknown) =>
    invoke<void>('open_window', { view, key: key ?? null, params: params ?? null }),
  takeParams: (token: string) => invoke<any>('take_params', { token }),
  extensionStatus: () => invoke<{ connected: boolean; browsers: string[]; port: number; bridgeError?: string | null }>('extension_status'),
  /** The key the browser extension must be given to talk to the app. */
  bridgeKey: () => invoke<string>('bridge_key'),
  /** Copy that key to the clipboard (kept out of the clipboard history, removed after a minute). */
  copyBridgeKey: () => invoke<void>('copy_bridge_key'),
  /** Replace the key; paired extensions stop working until they get the new one. */
  regenerateBridgeKey: () => invoke<string>('regenerate_bridge_key'),
  cancelPowerAction: () => invoke<void>('cancel_power_action'),
  getPowerCountdown: () => invoke<PowerCountdown | null>('get_power_countdown'),
  exportList: (path: string, onlyUnfinished: boolean) => invoke<number>('export_list', { path, onlyUnfinished }),
  readUrlList: (path: string) => invoke<string[]>('read_url_list', { path }),
  appInfo: () => invoke<{ name: string; version: string; os: string; arch: string; dataDir: string }>('app_info'),
  kvGet: (key: string) => invoke<string | null>('kv_get', { key }),
  kvSet: (key: string, value: string) => invoke<void>('kv_set', { key, value }),
  quit: () => invoke<void>('quit_app'),
  /** Send an event to every window. */
  broadcast: (event: string, payload: unknown) => invoke<void>('broadcast', { event, payload }),
  /** Font families installed on this computer, sorted by name. */
  listFonts: () => invoke<{ name: string; mono: boolean }[]>('list_fonts'),
  /** Write an exported colour profile; `path` must end in .odmtheme. */
  writeThemeFile: (path: string, contents: string) => invoke<void>('write_theme_file', { path, contents }),
  /**
   * A picture or font for a colour profile in the form the app keeps: a
   * picture is decoded and written out again, so only its pixels are left.
   * Rejects anything that is not a PNG picture or a font.
   */
  cleanThemeAsset: (dataUrl: string) => invoke<string>('clean_theme_asset', { dataUrl }),
  extensionDir: () => invoke<string | null>('extension_dir'),
  assignExtensions: (category: string, extensions: string[]) => invoke<void>('assign_extensions', { category, extensions }),
};

export function on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(event, (e) => cb(e.payload));
}

export function errText(e: unknown): string {
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object' && 'message' in e) return String((e as any).message);
  return String(e);
}
