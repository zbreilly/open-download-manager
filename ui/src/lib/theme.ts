import { on, type Settings } from './ipc';

let current = 'dark';
let mq: MediaQueryList | null = null;
/** Set while a colour profile is in use: it decides light or dark. */
let override: 'dark' | 'light' | null = null;

function apply(pref: string) {
  current = pref;
  const resolved = override ?? (pref === 'system'
    ? (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark')
    : pref);
  document.documentElement.dataset.theme = resolved;
}

/** A colour profile (lib/themes) takes over from the Light/Dark setting; null hands it back. */
export function setThemeOverride(base: 'dark' | 'light' | null) {
  override = base;
  apply(current);
}

/** Follow the theme setting live, including OS changes for "system". */
export function initTheme(initial: string) {
  apply(initial || 'dark');
  mq = matchMedia('(prefers-color-scheme: light)');
  mq.addEventListener('change', () => { if (current === 'system') apply('system'); });
  on<Settings>('settings_changed', (s) => {
    apply(s.theme);
    document.documentElement.dataset.density = s.density;
  });
}

/** Show a Light/Dark choice before it is saved (Settings). */
export function previewTheme(pref: string) {
  apply(pref);
}

export function setDensity(d: string) {
  document.documentElement.dataset.density = d;
}

export const isMac = navigator.userAgent.includes('Mac');
export const modKey = isMac ? '⌘' : 'Ctrl';
