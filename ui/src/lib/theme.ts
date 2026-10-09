import { on, type Settings } from './ipc';

let current = 'dark';
let mq: MediaQueryList | null = null;

function apply(pref: string) {
  current = pref;
  const resolved = pref === 'system'
    ? (matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark')
    : pref;
  document.documentElement.dataset.theme = resolved;
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

export function setDensity(d: string) {
  document.documentElement.dataset.density = d;
}

export const isMac = navigator.userAgent.includes('Mac');
export const modKey = isMac ? '⌘' : 'Ctrl';
