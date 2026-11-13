// Window frame: the app can draw its own title bar (so colour profiles style
// it) instead of the operating system's. Settings > Appearance > Title bar.
import { isMac } from './theme';
import type { Settings } from './ipc';

export const frame = $state({
  /** The app draws the title bar and window buttons. */
  custom: false,
  /** Text shown in the title bar. */
  title: 'Open Download Manager',
});

/** macOS keeps its own title bar: its window buttons can't be replaced well. */
export const wantsCustomFrame = (s: Pick<Settings, 'titleBar'> | null | undefined) => !isMac && (s?.titleBar ?? 'custom') !== 'native';

export const VIEW_TITLES: Record<string, string> = {
  main: 'Open Download Manager',
  add: 'Add download',
  progress: 'Download',
  scheduler: 'Queues & scheduler',
  settings: 'Settings',
  grabber: 'Site grabber',
  theme: 'Theme studio',
  about: 'About Open Download Manager',
};
