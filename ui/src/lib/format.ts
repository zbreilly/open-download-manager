// Formatting rules from HANDOFF §2.3
import type { Download, DownloadStatus } from './ipc';

const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;

export function bytes(n: number | null | undefined): string {
  if (n == null) return '—';
  if (n < MB) return `${(n / KB).toFixed(1)} KB`;
  if (n < GB) return `${(n / MB).toFixed(1)} MB`;
  return `${(n / GB).toFixed(2)} GB`;
}

export function speed(bps: number | null | undefined): string {
  if (!bps || bps < 1) return '—';
  return `${bytes(bps)}/s`;
}

export function pct(d: { receivedBytes: number; totalBytes: number | null; status: DownloadStatus }): number {
  if (d.status === 'complete') return 100;
  if (!d.totalBytes) return 0;
  return Math.min(99, Math.floor((d.receivedBytes / d.totalBytes) * 100));
}

export function eta(s: number | null | undefined): string {
  if (s == null || !isFinite(s)) return '—';
  if (s < 60) return `${Math.max(0, Math.round(s))}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${String(Math.round(s % 60)).padStart(2, '0')}s`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}m`;
  return '> 1 day';
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
const pad = (n: number) => String(n).padStart(2, '0');

export function added(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (isNaN(d.getTime())) return '—';
  const now = new Date();
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = (day(now) - day(d)) / 86400000;
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  if (diff === 0) return `Today ${hm}`;
  if (diff === 1) return `Yesterday ${hm}`;
  const md = `${MONTHS[d.getMonth()]} ${d.getDate()}`;
  return d.getFullYear() === now.getFullYear() ? md : `${md}, ${d.getFullYear()}`;
}

export function hhmm(iso: string | null | undefined): string {
  if (!iso) return '';
  const d = new Date(iso);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function statusLabel(d: Pick<Download, 'status' | 'scheduledAt'>): string {
  switch (d.status) {
    case 'downloading': return 'Downloading';
    case 'paused': return 'Paused';
    case 'queued': return d.scheduledAt ? `Starts ${hhmm(d.scheduledAt)}` : 'Queued';
    case 'error': return 'Failed';
    case 'complete': return 'Complete';
    case 'connecting': return 'Connecting';
    case 'assembling': return 'Finishing';
  }
}

export function statusColor(s: DownloadStatus): string {
  switch (s) {
    case 'downloading':
    case 'connecting':
    case 'assembling': return 'var(--accent)';
    case 'paused': return 'var(--warning)';
    case 'error': return 'var(--danger)';
    case 'complete': return 'var(--success)';
    default: return 'var(--text-muted)';
  }
}

/** Soft (tinted background) partner of statusColor. */
export function statusSoft(s: DownloadStatus): string {
  switch (s) {
    case 'downloading':
    case 'connecting':
    case 'assembling': return 'var(--accent-soft)';
    case 'paused': return 'var(--amber-soft)';
    case 'error': return 'var(--red-soft)';
    case 'complete': return 'var(--green-soft)';
    default: return 'var(--muted-soft)';
  }
}

export type Tone = 'blue' | 'green' | 'amber' | 'red' | 'violet' | 'teal' | 'pink' | 'orange' | 'muted';

const EXT_TONES: [Tone, string[]][] = [
  ['violet', ['mp4', 'mkv', 'avi', 'mov', 'webm', 'wmv', 'flv', 'm4v', 'mpg', 'mpeg', 'ts', '3gp']],
  ['pink', ['mp3', 'flac', 'wav', 'aac', 'ogg', 'm4a', 'opus', 'wma', 'aiff']],
  ['amber', ['zip', 'rar', '7z', 'tar', 'gz', 'bz2', 'xz', 'zst', 'tgz', 'cab']],
  ['blue', ['pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx', 'txt', 'epub', 'odt', 'csv', 'md', 'rtf', 'mobi']],
  ['teal', ['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp', 'tif', 'tiff', 'heic', 'avif', 'psd', 'ico']],
  ['green', ['exe', 'msi', 'dmg', 'pkg', 'deb', 'rpm', 'apk', 'appimage', 'bat', 'sh', 'jar']],
  ['orange', ['iso', 'img', 'bin', 'vhd', 'vmdk']],
];
const EXT_TONE = new Map<string, Tone>(EXT_TONES.flatMap(([t, exts]) => exts.map((e) => [e, t] as [string, Tone])));
/** Hue for a file type, so the list can be scanned by colour. */
export function extTone(ext: string | null | undefined): Tone {
  return EXT_TONE.get((ext ?? '').toLowerCase()) ?? 'muted';
}

/** Hue for a sidebar / category icon. */
export function iconTone(icon: string): Tone {
  switch (icon) {
    case 'cat-video': return 'violet';
    case 'cat-music': return 'pink';
    case 'cat-documents': return 'blue';
    case 'cat-compressed': return 'amber';
    case 'cat-programs': return 'green';
    case 'cat-images': return 'teal';
    case 'file-disc': return 'orange';
    case 'cat-grabber': return 'teal';
    case 'cat-all': return 'blue';
    case 'cat-active': return 'green';
    case 'cat-unfinished': return 'amber';
    case 'cat-finished': return 'teal';
    default: return 'orange';
  }
}

export function host(url: string): string {
  try { return new URL(url).host; } catch { return ''; }
}

export const MBPS = MB;

/** Largest speed limit accepted from a text field, in MB/s. */
export const MAX_LIMIT_MB = 1_000_000;

/**
 * Parse a "MB/s" text field into bytes per second. Returns null for anything
 * that is not a finite number above 0 (NaN, Infinity, negative, empty) or is
 * above MAX_LIMIT_MB, so callers can show an error instead of silently
 * turning a limit off.
 */
export function parseLimitMB(text: string): number | null {
  const v = Number(String(text).trim().replace(',', '.'));
  if (!Number.isFinite(v) || v <= 0 || v > MAX_LIMIT_MB) return null;
  return Math.max(1, Math.round(v * MB));
}

export const LIMIT_ERROR = `Enter a speed between 0 and ${MAX_LIMIT_MB.toLocaleString('en-US')} MB/s`;
