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

export function host(url: string): string {
  try { return new URL(url).host; } catch { return ''; }
}

export const MBPS = MB;
