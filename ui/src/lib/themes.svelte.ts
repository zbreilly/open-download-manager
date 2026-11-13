// Theme engine: colour profiles that restyle every window.
//
// A profile is a small JSON document: ~20 named colours, outline widths,
// fonts, and references to imported images (panel art, button icons, button
// skins). The engine derives every design token in tokens.css from it and
// sets them on <html>, so a profile overrides the built-in light/dark themes
// without touching the stylesheet.
//
// Storage (the app's key-value store, shared by all windows):
//   theme_profiles      JSON array of the user's saved profiles
//   theme_active        id of the profile in use ('' = built-in light/dark)
//   theme_asset.<id>    one imported file as a data: URL
// Windows stay in sync through two broadcast events: `theme_preview` (a
// profile being edited in the Theme studio) and `theme_changed` (re-read the
// saved state).
import { api, on } from './ipc';
import { setThemeOverride } from './theme';

// ------------------------------------------------------------------ model

export const COLOR_KEYS = [
  'window', 'chrome', 'panel', 'panel2', 'field', 'raised', 'popup',
  'text', 'textMuted', 'accent',
  'green', 'amber', 'red', 'violet', 'teal', 'pink', 'orange',
  'outlineWindow', 'outlinePanel', 'outlineControl', 'divider',
  'glow1', 'glow2',
] as const;
export type ColorKey = (typeof COLOR_KEYS)[number];
export type ThemeColors = Record<ColorKey, string>;

export const COLOR_GROUPS: { title: string; keys: { key: ColorKey; label: string; hint?: string }[] }[] = [
  { title: 'Windows & layers', keys: [
    { key: 'window', label: 'Window background' },
    { key: 'chrome', label: 'Header, sidebar, status bar' },
    { key: 'panel', label: 'Panels & dialogs' },
    { key: 'panel2', label: 'Cards, table header' },
    { key: 'field', label: 'Text fields' },
    { key: 'raised', label: 'Raised items' },
    { key: 'popup', label: 'Menus & pop-ups' },
  ] },
  { title: 'Outlines', keys: [
    { key: 'outlineWindow', label: 'Window outline' },
    { key: 'outlinePanel', label: 'Layer outline', hint: 'panels, menus, dialogs' },
    { key: 'outlineControl', label: 'Control outline', hint: 'buttons, fields, cards' },
    { key: 'divider', label: 'Row dividers' },
  ] },
  { title: 'Text', keys: [
    { key: 'text', label: 'Text' },
    { key: 'textMuted', label: 'Secondary text' },
  ] },
  { title: 'Accent & status', keys: [
    { key: 'accent', label: 'Accent', hint: 'primary buttons, progress, selection' },
    { key: 'green', label: 'Green', hint: 'complete, resume' },
    { key: 'amber', label: 'Amber', hint: 'paused, limits' },
    { key: 'red', label: 'Red', hint: 'failed, delete' },
    { key: 'violet', label: 'Violet' },
    { key: 'teal', label: 'Teal' },
    { key: 'pink', label: 'Pink' },
    { key: 'orange', label: 'Orange' },
  ] },
  { title: 'Background glow', keys: [
    { key: 'glow1', label: 'Glow, top left' },
    { key: 'glow2', label: 'Glow, bottom right' },
  ] },
];

export const ART_SLOTS = [
  { id: 'window', label: 'Window background' },
  { id: 'head', label: 'Header' },
  { id: 'side', label: 'Sidebar' },
  { id: 'list', label: 'Download list' },
  { id: 'details', label: 'Details panel' },
  { id: 'status', label: 'Status bar' },
  { id: 'dialog', label: 'Dialog windows' },
] as const;
export type ArtSlot = (typeof ART_SLOTS)[number]['id'];
export type ArtFit = 'cover' | 'contain' | 'stretch' | 'tile' | 'custom';
export interface PanelArt {
  asset: string;
  fit: ArtFit;
  /** 0..0.95: how much of the panel's own colour lies over the picture. */
  tint: number;
  /** Where the picture sits in the panel, as CSS background-position percentages (50, 50 = centred). */
  x: number;
  y: number;
  /** Picture width as a percentage of the panel's width; used by 'custom' and 'tile'. */
  size: number;
}
export const ART_DEFAULTS = { fit: 'cover' as ArtFit, tint: 0.55, x: 50, y: 50, size: 100 };

/** Buttons whose icon can be replaced. `icon` is the built-in icon name. */
export const BUTTON_SLOTS = [
  { icon: 'add-url', label: 'Add URL' },
  { icon: 'resume', label: 'Resume' },
  { icon: 'pause', label: 'Pause' },
  { icon: 'pause-all', label: 'Pause all' },
  { icon: 'delete', label: 'Delete' },
  { icon: 'delete-completed', label: 'Delete completed' },
  { icon: 'start-queue', label: 'Start queue' },
  { icon: 'stop-queue', label: 'Stop queue' },
  { icon: 'scheduler', label: 'Scheduler' },
  { icon: 'site-grabber', label: 'Grabber' },
  { icon: 'speed-limit', label: 'Speed limit' },
  { icon: 'more', label: 'More' },
  { icon: 'options', label: 'Settings' },
  { icon: 'open-file', label: 'Open file' },
  { icon: 'open-folder', label: 'Open folder' },
  { icon: 'copy-link', label: 'Copy link' },
] as const;

export interface ThemeProfile {
  v: 1;
  id: string;
  name: string;
  builtin?: boolean;
  /** Which built-in theme the profile sits on (native controls, scrollbars). */
  base: 'dark' | 'light';
  colors: ThemeColors;
  /** 0..1: how see-through the panels and the header/sidebar are. */
  panelOpacity: number;
  chromeOpacity: number;
  /** Background blur behind panels, px. */
  blur: number;
  /** 0..1: strength of the colour glows behind everything. */
  glow: number;
  /** No highlights or drop shadows. */
  flat: boolean;
  outline: { window: number; panel: number; control: number; neon: number; radius: number };
  fonts: { ui: string; mono: string; scale: number; uiAsset?: string; monoAsset?: string };
  art: Partial<Record<ArtSlot, PanelArt>>;
  /** Icon name -> imported image. `tint`: draw it in the button's colour. */
  icons: Record<string, { asset: string; tint: boolean }>;
  /** Background images for ordinary and primary buttons. */
  skin: { button?: string; primary?: string };
}

// ------------------------------------------------------------------ colour maths

export type RGB = [number, number, number];
const clamp = (x: number, a = 0, b = 1) => Math.min(b, Math.max(a, x));
export const isHex = (s: unknown): s is string => typeof s === 'string' && /^#[0-9a-fA-F]{6}$/.test(s);

export function hexToRgb(hex: string): RGB {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
export function rgbToHex([r, g, b]: RGB): string {
  const h = (x: number) => Math.round(clamp(x, 0, 255)).toString(16).padStart(2, '0');
  return `#${h(r)}${h(g)}${h(b)}`.toUpperCase();
}
/** h in degrees, s and v in 0..1. */
export function hsvToRgb(h: number, s: number, v: number): RGB {
  h = ((h % 360) + 360) % 360;
  const c = v * s, x = c * (1 - Math.abs(((h / 60) % 2) - 1)), m = v - c;
  const [r, g, b] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  return [(r + m) * 255, (g + m) * 255, (b + m) * 255];
}
export function rgbToHsv([r, g, b]: RGB): [number, number, number] {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b), d = max - min;
  let h = 0;
  if (d) {
    if (max === r) h = ((g - b) / d) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  return [h, max ? d / max : 0, max];
}
export function mix(a: string, b: string, t: number): string {
  const x = hexToRgb(a), y = hexToRgb(b);
  return rgbToHex([x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t, x[2] + (y[2] - x[2]) * t]);
}
export function rgba(hex: string, a: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r},${g},${b},${Math.round(clamp(a) * 1000) / 1000})`;
}
/** Relative luminance, 0..1. */
export function lum(hex: string): number {
  const f = (c: number) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
  const [r, g, b] = hexToRgb(hex);
  return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}
/** Black-ish or white text, whichever reads better on `bg`. */
export const readable = (bg: string) => (lum(bg) > 0.36 ? '#05070C' : '#FFFFFF');
/** Contrast ratio of two colours, 1 (identical) to 21 (black on white). */
export function contrast(a: string, b: string): number {
  const x = lum(a), y = lum(b);
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

// A profile must never make the app unreadable: text, secondary text and the
// warning colours keep a minimum contrast against every surface they are
// drawn on. A colour that falls below it is shown as black or white instead
// (whichever reads better), so warnings and labels cannot be hidden by a
// profile, whether it was imported or made by hand.
const SURFACES = ['window', 'chrome', 'panel', 'panel2', 'field', 'popup'] as const;
const MIN_CONTRAST: [ColorKey, number][] = [['text', 3], ['textMuted', 2], ['red', 2], ['amber', 2]];
const BLACK_INK = '#05070C', WHITE_INK = '#FFFFFF';

/**
 * The colours as the app shows them, and which ones had to be replaced to
 * stay readable.
 */
export function legible(c: ThemeColors): { colors: ThemeColors; fixed: ColorKey[] } {
  const worst = (fg: string) => Math.min(...SURFACES.map((s) => contrast(fg, c[s])));
  const colors = { ...c };
  const fixed: ColorKey[] = [];
  for (const [key, min] of MIN_CONTRAST) {
    if (worst(c[key]) >= min) continue;
    colors[key] = worst(BLACK_INK) >= worst(WHITE_INK) ? BLACK_INK : WHITE_INK;
    fixed.push(key);
  }
  return { colors, fixed };
}

// ------------------------------------------------------------------ built-in profiles

const DARK: ThemeColors = {
  window: '#06080C', chrome: '#141922', panel: '#1B212C', panel2: '#1F2632', field: '#0B0E14', raised: '#2B3444', popup: '#1E2531',
  text: '#F4F6FA', textMuted: '#A6B0C0', accent: '#5B9DFF',
  green: '#3DD68C', amber: '#FFB224', red: '#FF6B78', violet: '#A98BFF', teal: '#2DD4BF', pink: '#F57AB8', orange: '#FF9548',
  outlineWindow: '#343D4D', outlinePanel: '#3A4352', outlineControl: '#343D4D', divider: '#252C39',
  glow1: '#4078FF', glow2: '#9664FF',
};
const LIGHT: ThemeColors = {
  window: '#D3DAE8', chrome: '#FFFFFF', panel: '#FFFFFF', panel2: '#EEF1F7', field: '#F4F6FA', raised: '#E2E7F0', popup: '#FFFFFF',
  text: '#0A0E16', textMuted: '#465064', accent: '#2257E6',
  green: '#0B8043', amber: '#B45B00', red: '#CC2B3B', violet: '#6A3FD9', teal: '#0A7C76', pink: '#C22679', orange: '#C2510C',
  outlineWindow: '#BEC7D6', outlinePanel: '#C4CCDA', outlineControl: '#BEC7D6', divider: '#DCE2EC',
  glow1: '#2257E6', glow2: '#7C4DFF',
};

function make(id: string, name: string, base: 'dark' | 'light', colors: Partial<ThemeColors>, rest: Partial<ThemeProfile> = {}): ThemeProfile {
  return {
    v: 1, id, name, builtin: true, base,
    colors: { ...(base === 'dark' ? DARK : LIGHT), ...colors },
    panelOpacity: base === 'dark' ? 0.78 : 0.86, chromeOpacity: base === 'dark' ? 0.74 : 0.7, blur: 18, glow: 1, flat: false,
    outline: { window: 0, panel: 1, control: 1, neon: 0, radius: 1 },
    fonts: { ui: 'IBM Plex Sans', mono: 'JetBrains Mono', scale: 1 },
    art: {}, icons: {}, skin: {},
    ...rest,
  };
}
const SOLID = { panelOpacity: 1, chromeOpacity: 1, blur: 0, glow: 0, flat: true };
const BLACK = { window: '#000000', chrome: '#000000', panel: '#000000', panel2: '#0C0C0C', field: '#000000', raised: '#1A1A1A', popup: '#000000', text: '#FFFFFF' };
const WHITE = { window: '#FFFFFF', chrome: '#FFFFFF', panel: '#FFFFFF', panel2: '#F3F3F3', field: '#FFFFFF', raised: '#E8E8E8', popup: '#FFFFFF', text: '#000000' };

/** The two profiles the app ships as its light and dark themes. */
export const defaultProfile = (base: 'dark' | 'light') => make('default', base === 'dark' ? 'Default dark' : 'Default light', base, {});

export const BUILTIN: ThemeProfile[] = [
  make('all-black', 'All black', 'dark',
    { ...BLACK, textMuted: '#B4B4B4', outlineWindow: '#2A2A2A', outlinePanel: '#2A2A2A', outlineControl: '#333333', divider: '#1C1C1C' }, SOLID),
  make('all-white', 'All white', 'light',
    { ...WHITE, textMuted: '#555555', outlineWindow: '#D9D9D9', outlinePanel: '#D9D9D9', outlineControl: '#C8C8C8', divider: '#E6E6E6' }, SOLID),
  make('contrast-dark', 'High contrast dark', 'dark',
    { ...BLACK, panel2: '#000000', textMuted: '#FFFFFF', accent: '#FFE600', green: '#3DFF7C', amber: '#FFC800', red: '#FF5A5A', violet: '#C9A6FF', teal: '#4DF5FF', pink: '#FF8AD8', orange: '#FFA24A',
      outlineWindow: '#FFFFFF', outlinePanel: '#FFFFFF', outlineControl: '#FFFFFF', divider: '#8A8A8A' },
    { ...SOLID, outline: { window: 2, panel: 2, control: 2, neon: 0, radius: 0.5 } }),
  make('contrast-light', 'High contrast light', 'light',
    { ...WHITE, panel2: '#FFFFFF', textMuted: '#000000', accent: '#0033CC', green: '#006B2D', amber: '#8A4500', red: '#B00018', violet: '#4B1FB8', teal: '#005F5A', pink: '#9C0F5F', orange: '#933B00',
      outlineWindow: '#000000', outlinePanel: '#000000', outlineControl: '#000000', divider: '#6E6E6E' },
    { ...SOLID, outline: { window: 2, panel: 2, control: 2, neon: 0, radius: 0.5 } }),
  make('neon-black', 'Black with neon outlines', 'dark',
    { ...BLACK, textMuted: '#9FB4C4', accent: '#00F0FF', green: '#39FF14', amber: '#FFE600', red: '#FF2D55', violet: '#B026FF', teal: '#00FFC8', pink: '#FF2BD6', orange: '#FF7A00',
      outlineWindow: '#FF2BD6', outlinePanel: '#00F0FF', outlineControl: '#0B6E78', divider: '#10262B' },
    { ...SOLID, outline: { window: 2, panel: 1, control: 1, neon: 12, radius: 1 } }),
  make('neon-white', 'White with neon outlines', 'light',
    { ...WHITE, textMuted: '#4A4458', accent: '#D600B0', green: '#00A83C', amber: '#D97A00', red: '#F0003C', violet: '#7A00FF', teal: '#00A5A0', pink: '#FF00C8', orange: '#F25C00',
      outlineWindow: '#7A00FF', outlinePanel: '#FF00C8', outlineControl: '#C9A2E8', divider: '#F0DDF5' },
    { ...SOLID, outline: { window: 2, panel: 1, control: 1, neon: 10, radius: 1 } }),
];

// ------------------------------------------------------------------ validation
// Profiles come from disk and from imported files: nothing in one is trusted.
// Every value that ends up in CSS is checked against a strict shape.

const ASSET_RE = /^data:(image\/png|image\/svg\+xml|font\/(ttf|otf|woff2?)|application\/(x-font-(ttf|otf|woff)|font-(sfnt|woff2?)|octet-stream));base64,[A-Za-z0-9+/]+=*$/;
export const isAssetUrl = (s: unknown): s is string => typeof s === 'string' && s.length <= MAX_ASSET && ASSET_RE.test(s);
export const isImageUrl = (s: unknown): s is string => isAssetUrl(s) && s.startsWith('data:image/');
/** Largest imported file, as a data: URL (about 6 MB of file). */
export const MAX_ASSET = 8_400_000;
const ID_RE = /^[A-Za-z0-9_-]{1,40}$/;
const ICON_SLOTS = new Set<string>(BUTTON_SLOTS.map((b) => b.icon));
// a font family name goes into CSS inside double quotes: anything that could end the string or the rule is out
const FONT_RE = /^[^"\\;{}<>\u0000-\u001f]{1,80}$/;
export const isFontName = (s: string) => FONT_RE.test(s.trim()) && s.trim().length > 0;
const num = (x: unknown, lo: number, hi: number, d: number) => (typeof x === 'number' && isFinite(x) ? clamp(x, lo, hi) : d);
const assetId = (x: unknown) => (typeof x === 'string' && ID_RE.test(x) ? x : undefined);

/** A clean ThemeProfile from untrusted data, or null. */
export function sanitizeProfile(raw: unknown): ThemeProfile | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Record<string, any>;
  const base = r.base === 'light' ? 'light' : 'dark';
  const d = defaultProfile(base);
  if (typeof r.id !== 'string' || !ID_RE.test(r.id)) return null;
  const p: ThemeProfile = { ...d, id: r.id, builtin: false, name: String(r.name ?? 'Untitled').replace(/[\u0000-\u001f]/g, '').slice(0, 60) || 'Untitled' };
  for (const k of COLOR_KEYS) if (isHex(r.colors?.[k])) p.colors[k] = r.colors[k].toUpperCase();
  p.panelOpacity = num(r.panelOpacity, 0.2, 1, d.panelOpacity);
  p.chromeOpacity = num(r.chromeOpacity, 0.2, 1, d.chromeOpacity);
  p.blur = num(r.blur, 0, 40, d.blur);
  p.glow = num(r.glow, 0, 1, d.glow);
  p.flat = !!r.flat;
  p.outline = {
    window: num(r.outline?.window, 0, 8, 0), panel: num(r.outline?.panel, 0, 4, 1), control: num(r.outline?.control, 0, 3, 1),
    neon: num(r.outline?.neon, 0, 30, 0), radius: num(r.outline?.radius, 0, 2, 1),
  };
  const font = (x: unknown, dflt: string) => (typeof x === 'string' && FONT_RE.test(x.trim()) ? x.trim() : dflt);
  p.fonts = {
    ui: font(r.fonts?.ui, d.fonts.ui), mono: font(r.fonts?.mono, d.fonts.mono), scale: num(r.fonts?.scale, 0.85, 1.3, 1),
    uiAsset: assetId(r.fonts?.uiAsset), monoAsset: assetId(r.fonts?.monoAsset),
  };
  p.art = {};
  for (const s of ART_SLOTS) {
    const a = r.art?.[s.id];
    const id = assetId(a?.asset);
    if (id) {
      const fit: ArtFit = ['cover', 'contain', 'stretch', 'tile', 'custom'].includes(a.fit) ? a.fit : 'cover';
      p.art[s.id] = {
        asset: id, fit, tint: num(a.tint, 0, 0.95, 0.5),
        x: num(a.x, -100, 200, 50), y: num(a.y, -100, 200, 50),
        // a tile with no size keeps the picture's own size (0)
        size: num(a.size, fit === 'tile' ? 0 : 5, 400, fit === 'tile' ? 0 : 100),
      };
    }
  }
  // only the listed buttons can get another picture: a profile must not be
  // able to redraw status icons or make one action look like another
  p.icons = {};
  for (const [name, v] of Object.entries(r.icons ?? {})) {
    const id = assetId((v as any)?.asset);
    if (id && ICON_SLOTS.has(name)) p.icons[name] = { asset: id, tint: !!(v as any).tint };
  }
  p.skin = { button: assetId(r.skin?.button), primary: assetId(r.skin?.primary) };
  return p;
}

/** Every asset id a profile refers to. */
export function assetIds(p: ThemeProfile): string[] {
  const ids = [p.fonts.uiAsset, p.fonts.monoAsset, p.skin.button, p.skin.primary,
    ...Object.values(p.art).map((a) => a?.asset), ...Object.values(p.icons).map((i) => i.asset)];
  return [...new Set(ids.filter((x): x is string => !!x))];
}

// ------------------------------------------------------------------ assets

const assets = new Map<string, string>();
const newId = (prefix: string) => prefix + Math.random().toString(36).slice(2, 10) + Date.now().toString(36).slice(-4);
export const newProfileId = () => newId('p');

// Every stored file is listed in an index, so files that no saved profile
// uses (an import that was never saved, a picture that was replaced) can be
// found and removed instead of staying in the database for good.
const ASSET_INDEX = 'theme_assets';
async function readIndex(): Promise<string[]> {
  try {
    const list = JSON.parse((await api.kvGet(ASSET_INDEX)) || '[]');
    return Array.isArray(list) ? list.filter((x): x is string => typeof x === 'string' && ID_RE.test(x)) : [];
  } catch {
    return [];
  }
}

/** Store an imported file; returns its asset id. */
export async function putAsset(dataUrl: string): Promise<string> {
  if (!isAssetUrl(dataUrl)) throw new Error('That file type is not supported, or the file is too large');
  const id = newId('a');
  // listed before it is stored: a file can be listed and missing, never stored and unlisted
  await api.kvSet(ASSET_INDEX, JSON.stringify([...(await readIndex()), id]));
  assets.set(id, dataUrl);
  await api.kvSet(`theme_asset.${id}`, dataUrl);
  return id;
}

/**
 * Remove every stored file that is not in `keep`. Call only when nothing
 * unsaved can refer to a file: right after saving, or before the Theme
 * studio has any draft.
 */
export async function collectAssets(keep: Iterable<string>) {
  const wanted = new Set(keep);
  const listed = await readIndex();
  await dropAssets(listed.filter((id) => !wanted.has(id)));
  await api.kvSet(ASSET_INDEX, JSON.stringify([...wanted]));
}
export const assetUrl = (id: string | undefined) => (id ? assets.get(id) : undefined);

async function loadAssets(ids: string[]) {
  await Promise.all(ids.filter((id) => !assets.has(id)).map(async (id) => {
    const v = await api.kvGet(`theme_asset.${id}`).catch(() => null);
    if (isAssetUrl(v)) assets.set(id, v);
  }));
}
/** Free the stored copies of assets no saved profile uses any more. */
async function dropAssets(ids: string[]) {
  for (const id of ids) {
    assets.delete(id);
    await api.kvSet(`theme_asset.${id}`, '').catch(() => {});
  }
}

// ------------------------------------------------------------------ applying

/** Reactive state the UI reads: icon overrides, and what is saved. */
export const themeState = $state({
  icons: {} as Record<string, { url: string; tint: boolean }>,
  profiles: [] as ThemeProfile[],
  activeId: '',
});

const STYLE_ID = 'odm-theme';
let setProps: string[] = [];

function tokens(p: ThemeProfile): Record<string, string> {
  const c = legible(p.colors).colors, dark = p.base === 'dark';
  const soft = dark ? 0.17 : 0.13;
  const t: Record<string, string> = {
    '--bg': c.window,
    '--surface': c.panel, '--surface-2': c.panel2, '--surface-sunken': c.field, '--raised': c.raised,
    '--border': c.outlineControl, '--border-subtle': c.divider,
    '--text': c.text, '--text-muted': c.textMuted,
    '--accent': c.accent, '--accent-hi': mix(c.accent, '#FFFFFF', 0.2), '--accent-lo': mix(c.accent, '#000000', 0.16),
    '--on-accent': readable(c.accent), '--accent-soft': rgba(c.accent, soft + 0.03), '--accent-glow': rgba(c.accent, p.flat ? 0 : 0.35),
    '--row-selected': rgba(c.accent, soft),
    '--hover': rgba(c.text, 0.075), '--tint': rgba(c.text, 0.04),
    '--track': mix(c.panel, c.text, 0.17), '--switch-off': mix(c.panel, c.text, 0.3),
    '--muted-soft': rgba(c.textMuted, 0.14),
    '--glass': rgba(c.panel, p.panelOpacity), '--glass-chrome': rgba(c.chrome, p.chromeOpacity), '--glass-pop': rgba(c.popup, Math.max(0.9, p.panelOpacity)),
    '--glass-border': c.outlinePanel,
    '--glass-hi': p.flat ? 'transparent' : dark ? 'rgba(255,255,255,0.07)' : 'rgba(255,255,255,0.95)',
    '--glow-1': rgba(c.glow1, 0.26 * p.glow), '--glow-2': rgba(c.glow2, 0.2 * p.glow), '--glow-3': rgba(c.teal, 0.1 * p.glow),
    '--scrim': rgba(mix(c.window, '#000000', 0.6), dark ? 0.6 : 0.36),
    '--btn-bg': dark ? rgba(c.text, 0.055) : rgba(c.panel, 0.75),
    '--on-danger': readable(c.red),
    '--blur': p.blur > 0 ? `blur(${p.blur}px) saturate(150%)` : 'none',
    '--outline-w': `${p.outline.panel}px`, '--outline-control-w': `${p.outline.control}px`,
    '--outline-window-w': `${p.outline.window}px`, '--outline-window': c.outlineWindow,
    '--outline-neon': p.outline.neon > 0 ? `0 0 ${p.outline.neon}px ${rgba(c.outlinePanel, 0.75)}` : '0 0 0 transparent',
    '--outline-window-neon': p.outline.neon > 0 ? `inset 0 0 ${p.outline.neon * 1.5}px ${rgba(c.outlineWindow, 0.7)}` : '0 0 0 transparent',
    '--font-ui': `"${p.fonts.uiAsset ? 'ODM Custom UI' : p.fonts.ui}", "IBM Plex Sans", system-ui, sans-serif`,
    '--font-mono': `"${p.fonts.monoAsset ? 'ODM Custom Mono' : p.fonts.mono}", "JetBrains Mono", ui-monospace, monospace`,
  };
  if (p.flat) { t['--shadow-card'] = '0 0 0 transparent'; t['--shadow-pop'] = '0 0 0 transparent'; }
  for (const h of ['green', 'amber', 'red', 'violet', 'teal', 'pink', 'orange'] as const) {
    t[`--${h}`] = c[h];
    t[`--${h}-soft`] = rgba(c[h], soft);
  }
  const radius = { sm: 5, md: 8, lg: 10, xl: 12 };
  for (const [k, v] of Object.entries(radius)) t[`--r-${k}`] = `${Math.round(v * p.outline.radius * 10) / 10}px`;
  const fs = { xs: 11, sm: 12, md: 13, lg: 14, xl: 15, hero: 40 };
  for (const [k, v] of Object.entries(fs)) t[`--fs-${k}`] = `${Math.round(v * p.fonts.scale * 10) / 10}px`;
  return t;
}

const ART_CSS: Record<ArtSlot, { sel: string; color: (c: ThemeColors) => string }> = {
  window: { sel: 'body', color: (c) => c.window },
  head: { sel: '.art-head', color: (c) => c.chrome },
  side: { sel: '.art-side', color: (c) => c.chrome },
  list: { sel: '.art-list', color: (c) => c.panel },
  details: { sel: '.art-details', color: (c) => c.panel },
  status: { sel: '.art-status', color: (c) => c.chrome },
  dialog: { sel: '.dialog, .art-dialog', color: (c) => c.panel },
};
/** The CSS `background` for a piece of panel art: the panel colour washed over the picture, framed as the profile says. */
export function artBackground(p: ThemeProfile, slot: ArtSlot, url: string): string {
  const a = p.art[slot]!;
  const wash = rgba(ART_CSS[slot].color(p.colors), a.tint);
  const r = (n: number) => Math.round(n * 10) / 10;
  const size = a.fit === 'cover' ? 'cover' : a.fit === 'contain' ? 'contain' : a.fit === 'stretch' ? '100% 100%' : a.size > 0 ? `${r(a.size)}% auto` : 'auto';
  return `linear-gradient(${wash},${wash}),url("${url}") ${r(a.x)}% ${r(a.y)}% / ${size} ${a.fit === 'tile' ? 'repeat' : 'no-repeat'}`;
}

function styleSheet(p: ThemeProfile): string {
  const out: string[] = [];
  const face = (family: string, id?: string) => {
    const u = assetUrl(id);
    if (u) out.push(`@font-face{font-family:"${family}";src:url("${u}");font-weight:100 900;font-display:block}`);
  };
  face('ODM Custom UI', p.fonts.uiAsset);
  face('ODM Custom Mono', p.fonts.monoAsset);
  for (const s of ART_SLOTS) {
    const a = p.art[s.id], u = assetUrl(a?.asset);
    if (!a || !isImageUrl(u)) continue;
    out.push(`${ART_CSS[s.id].sel}{background:${artBackground(p, s.id, u)} !important}`);
  }
  const skin = (sel: string, id?: string) => {
    const u = assetUrl(id);
    if (isImageUrl(u)) out.push(`${sel}{background:url("${u}") center / 100% 100% no-repeat !important;border-color:transparent !important;box-shadow:none !important}${sel}:hover:not(:disabled){filter:brightness(1.12)}`);
  };
  skin('.btn:not(.link):not(.primary)', p.skin.button);
  skin('.btn.primary', p.skin.primary);
  return out.join('\n');
}

/** Put a profile on this window. `null` returns to the built-in themes. */
export async function applyProfile(p: ThemeProfile | null) {
  const root = document.documentElement;
  for (const k of setProps) root.style.removeProperty(k);
  setProps = [];
  let style = document.getElementById(STYLE_ID) as HTMLStyleElement | null;
  if (!p) {
    style?.remove();
    themeState.icons = {};
    setThemeOverride(null);
    return;
  }
  await loadAssets(assetIds(p));
  const t = tokens(p);
  for (const [k, v] of Object.entries(t)) root.style.setProperty(k, v);
  setProps = Object.keys(t);
  if (!style) {
    style = document.createElement('style');
    style.id = STYLE_ID;
    document.head.appendChild(style);
  }
  style.textContent = styleSheet(p);
  const icons: Record<string, { url: string; tint: boolean }> = {};
  for (const [name, v] of Object.entries(p.icons)) {
    const u = assetUrl(v.asset);
    if (isImageUrl(u)) icons[name] = { url: u, tint: v.tint };
  }
  themeState.icons = icons;
  setThemeOverride(p.base);
}

// ------------------------------------------------------------------ saved state

export const findProfile = (id: string): ThemeProfile | null =>
  BUILTIN.find((b) => b.id === id) ?? themeState.profiles.find((x) => x.id === id) ?? null;

async function readSaved() {
  const [rawList, active] = await Promise.all([api.kvGet('theme_profiles').catch(() => null), api.kvGet('theme_active').catch(() => null)]);
  let list: ThemeProfile[] = [];
  try {
    const arr = JSON.parse(rawList || '[]');
    if (Array.isArray(arr)) list = arr.map(sanitizeProfile).filter((x): x is ThemeProfile => !!x);
  } catch {}
  themeState.profiles = list;
  themeState.activeId = active && findProfile(active) ? active : '';
}

/** Load the saved profile and keep this window in step with the others. */
export async function initThemeEngine() {
  try {
    await readSaved();
    await applyProfile(findProfile(themeState.activeId));
  } catch {}
  on<unknown>('theme_preview', (raw) => {
    if (raw == null) return void applyProfile(null);
    const r = raw as Record<string, unknown>;
    // a built-in being previewed keeps its flag (sanitizeProfile clears it)
    const p = sanitizeProfile(raw);
    if (p) applyProfile({ ...p, builtin: !!r.builtin });
  });
  on('theme_changed', async () => {
    await readSaved();
    await applyProfile(findProfile(themeState.activeId));
  });
}

/** Show a profile in every window without saving it. */
export function previewEverywhere(p: ThemeProfile | null) {
  return api.broadcast('theme_preview', p);
}

/** Save the user's profiles and choose the one in use; every window follows. */
export async function saveThemes(profiles: ThemeProfile[], activeId: string) {
  const clean = profiles.filter((p) => !p.builtin);
  const keep = new Set(clean.flatMap(assetIds));
  const gone = themeState.profiles.flatMap(assetIds).filter((id) => !keep.has(id));
  await api.kvSet('theme_profiles', JSON.stringify(clean));
  await api.kvSet('theme_active', activeId);
  await dropAssets([...new Set(gone)]);
  // everything the studio holds is saved now: whatever else is stored is unused
  await collectAssets(keep);
  await api.broadcast('theme_changed', null);
}

// ------------------------------------------------------------------ files

export interface ThemeFile { format: 'odm-theme'; version: 1; profile: ThemeProfile; assets: Record<string, string> }

/** A profile and the files it uses, as the text of a .odmtheme file. */
export async function exportTheme(p: ThemeProfile): Promise<string> {
  const ids = assetIds(p);
  await loadAssets(ids);
  const file: ThemeFile = { format: 'odm-theme', version: 1, profile: { ...p, builtin: false }, assets: {} };
  for (const id of ids) {
    const u = assetUrl(id);
    if (u) file.assets[id] = u;
  }
  return JSON.stringify(file);
}

/** Read a .odmtheme file. The profile gets a new id; its files are stored. */
export async function importTheme(text: string): Promise<ThemeProfile> {
  let file: any;
  try { file = JSON.parse(text); } catch { throw new Error('That is not a theme file'); }
  if (file?.format !== 'odm-theme' || !file.profile) throw new Error('That is not a theme file');
  const p = sanitizeProfile({ ...file.profile, id: newProfileId() });
  if (!p) throw new Error('The theme file is damaged');
  // store each referenced file under a fresh id; drop references to missing or invalid ones
  const map = new Map<string, string>();
  for (const id of assetIds(p)) {
    const u = file.assets?.[id];
    if (isAssetUrl(u)) map.set(id, await putAsset(u));
  }
  const re = (id?: string) => (id ? map.get(id) : undefined);
  p.fonts.uiAsset = re(p.fonts.uiAsset);
  p.fonts.monoAsset = re(p.fonts.monoAsset);
  p.skin = { button: re(p.skin.button), primary: re(p.skin.primary) };
  for (const s of ART_SLOTS) {
    const a = p.art[s.id];
    if (!a) continue;
    const id = re(a.asset);
    if (id) a.asset = id; else delete p.art[s.id];
  }
  for (const [name, v] of Object.entries(p.icons)) {
    const id = re(v.asset);
    if (id) v.asset = id; else delete p.icons[name];
  }
  return p;
}
