// Open Download Manager — browser extension background.
// Chromium (MV3 service worker) and Firefox (MV2 event page).
//
// * Takes over matching browser downloads and hands them to the app
//   (with cookies, referrer and user agent so authenticated links work).
// * Bypass: hold the configured key (Alt by default) while clicking, or
//   pause capture for 10 minutes from the toolbar popup.
// * Detects media files loaded by pages and lists them in the popup.
// * Context menus for links, media, selections and "all links on page".
"use strict";

const B = globalThis.browser ?? globalThis.chrome;
const IS_FIREFOX = typeof globalThis.browser !== "undefined" && navigator.userAgent.includes("Firefox");
const DEFAULT_PORT = 8901;

// sealed messages (ODMPairing): a service worker loads the file itself,
// Firefox lists it in the manifest
if (!globalThis.ODMPairing && typeof importScripts === "function") importScripts("pairing.js");
const Pairing = globalThis.ODMPairing;

// the pairing key is for the extension's own pages only, not content scripts
try { B.storage.local.setAccessLevel?.({ accessLevel: "TRUSTED_CONTEXTS" })?.catch?.(() => {}); } catch {}

const state = {
  // the app answered with the pairing key: only then is anything sent to it
  connected: false,
  // something that says it is the app listens on the port
  appRunning: false,
  // a pairing key is stored
  paired: false,
  // the stored key is not the app's key
  keyRejected: false,
  capture: { enabled: true, paused: false, extensions: [], bypassKey: "alt", minSizeKb: 0 },
  lastCheck: 0,
  bypassUntil: 0,
  media: new Map(), // tabId -> Map(url -> {url, mime, size, name})
};

function browserName() {
  const ua = navigator.userAgent;
  if (ua.includes("Firefox")) return "Firefox";
  if (ua.includes("Edg/")) return "Edge";
  if (ua.includes("OPR/")) return "Opera";
  if (ua.includes("Vivaldi")) return "Vivaldi";
  if (navigator.brave) return "Brave";
  return "Chrome";
}

async function port() {
  try {
    const d = await B.storage.local.get({ port: DEFAULT_PORT });
    return Number(d.port) || DEFAULT_PORT;
  } catch {
    return DEFAULT_PORT;
  }
}

/** The listener did not prove it holds the pairing key (or no key is stored). */
class PairingError extends Error {}

let keyCache = { hex: "", key: null };

/** The stored pairing key as a CryptoKey, or null when not paired. */
async function pairingKey() {
  let hex = "";
  try {
    hex = Pairing.parseKey((await B.storage.local.get({ pairKey: "" })).pairKey) || "";
  } catch {}
  state.paired = !!hex;
  if (!hex) return null;
  if (keyCache.hex !== hex) keyCache = { hex, key: await Pairing.importKey(hex) };
  return keyCache.key;
}

async function request(path, init, timeoutMs) {
  const p = await port();
  const ctl = new AbortController();
  const t = setTimeout(() => ctl.abort(), timeoutMs);
  try {
    return await fetch(`http://127.0.0.1:${p}${path}`, { ...init, signal: ctl.signal });
  } finally {
    clearTimeout(t);
  }
}

/**
 * Ask the app to do `op`. The request is sealed with the pairing key, so
 * only the app can read it, and the answer must be sealed with the same key
 * for this very request: anything else on the port gets nothing and is not
 * believed. Throws PairingError when not paired or the key is not accepted.
 */
async function rpc(op, body) {
  const key = await pairingKey();
  if (!key) throw new PairingError("not paired");
  const sealed = await Pairing.sealRequest(key, op, body ?? null);
  const r = await request("/api/rpc", {
    method: "POST",
    headers: { "content-type": "application/json", "x-odm-browser": browserName() },
    body: sealed.envelope,
  }, 5000);
  if (r.status === 401) throw new PairingError("pairing key not accepted");
  if (!r.ok) throw new Error(`HTTP ${r.status}`);
  let answer;
  try {
    answer = await Pairing.openResponse(key, sealed.nonce, await r.text());
  } catch {
    throw new PairingError("the answer was not sealed with the pairing key");
  }
  if (!answer || answer.ok === false) throw new Error((answer && answer.error) || "request failed");
  return answer;
}

/** Is something that calls itself the app listening? (Unsealed; tells nothing else.) */
async function appListening() {
  try {
    const r = await request("/api/ping", { method: "GET" }, 2500);
    return r.ok;
  } catch {
    return false;
  }
}

async function ping() {
  try {
    const info = await rpc("ping");
    state.connected = true;
    state.appRunning = true;
    state.keyRejected = false;
    state.capture = info.capture || state.capture;
  } catch (e) {
    state.connected = false;
    if (e instanceof PairingError) {
      state.appRunning = await appListening();
      state.keyRejected = state.paired && state.appRunning;
    } else {
      state.appRunning = false;
      state.keyRejected = false;
    }
  }
  state.lastCheck = Date.now();
  updateIcon();
  return state.connected;
}

async function ensureFresh() {
  if (Date.now() - state.lastCheck > 15000) await ping();
  return state.connected;
}

function updateIcon() {
  const action = B.action ?? B.browserAction;
  if (!action) return;
  const off = !state.connected || !state.capture.enabled || state.capture.paused;
  const path = off
    ? { 16: "icons/icon-16-paused.png", 32: "icons/icon-32-paused.png" }
    : { 16: "icons/icon-16.png", 32: "icons/icon-32.png" };
  try { action.setIcon({ path }); } catch {}
  const title = !state.connected
    ? state.appRunning
      ? "Open Download Manager — not paired with the app"
      : "Open Download Manager — app not running"
    : state.capture.paused
      ? "Open Download Manager — capture paused"
      : "Open Download Manager";
  try { action.setTitle({ title }); } catch {}
}

/**
 * Cookie header for `url` from the given cookie store (Firefox containers /
 * private windows); the default store when `storeId` is empty.
 */
async function cookiesFor(url, storeId) {
  try {
    const q = { url };
    if (storeId) q.storeId = storeId;
    const list = await B.cookies.getAll(q);
    return list.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return "";
  }
}

/** Cookie store of a tab: Firefox containers carry it; Chromium incognito tabs are looked up. */
async function storeIdForTab(tab) {
  if (!tab) return undefined;
  if (tab.cookieStoreId) return tab.cookieStoreId;
  if (!tab.incognito) return undefined;
  try {
    const stores = await B.cookies.getAllCookieStores();
    return stores.find((s) => (s.tabIds || []).includes(tab.id))?.id;
  } catch {
    return undefined;
  }
}

/** [{url, cookies}] with each URL's own cookies (cookies omitted when none). */
async function cookieItems(urls, storeId) {
  const out = [];
  const seen = new Map();
  for (let i = 0; i < urls.length; i += 32) {
    const chunk = urls.slice(i, i + 32);
    const got = await Promise.all(chunk.map((u) => {
      if (!seen.has(u)) seen.set(u, cookiesFor(u, storeId));
      return seen.get(u);
    }));
    chunk.forEach((u, k) => out.push(got[k] ? { url: u, cookies: got[k] } : { url: u }));
  }
  return out;
}

/** Send one or more URLs to the app. Returns true on success. */
async function sendToApp({ url, urls, referrer, fileName, totalBytes, mime, storeId }) {
  const list = (urls || []).filter((u) => typeof u === "string" && u);
  if (!url && !list.length) return false;
  // nothing is collected or sent unless the app has answered with the
  // pairing key
  if (!state.connected && !(await ping())) return false;
  const items = list.length ? await cookieItems(list, storeId) : [];
  const cookies = url ? await cookiesFor(url, storeId) : "";
  try {
    await rpc("add", {
      url: url || "",
      // each link with its own cookies
      items,
      referrer: referrer || "",
      fileName: fileName || null,
      cookies,
      userAgent: navigator.userAgent,
      totalBytes: totalBytes > 0 ? totalBytes : null,
      mime: mime || null,
    });
    state.connected = true;
    return true;
  } catch {
    state.connected = false;
    updateIcon();
    return false;
  }
}

// ---------------------------------------------------------------- capture

function extOf(nameOrUrl) {
  let s = nameOrUrl || "";
  try { s = new URL(s).pathname; } catch {}
  s = s.split(/[\\/]/).pop() || "";
  const i = s.lastIndexOf(".");
  return i > 0 ? s.slice(i + 1).toLowerCase() : "";
}

function shouldCapture(item, name) {
  const c = state.capture;
  if (!state.connected || !c.enabled || c.paused) return false;
  if (Date.now() < state.bypassUntil) return false;
  if (item.incognito) return false; // private windows: the browser handles it
  const url = item.finalUrl || item.url || "";
  if (!/^https?:/i.test(url)) return false; // blob:, data:, file: stay in the browser
  const ext = extOf(name) || extOf(url);
  if (!ext || !c.extensions.includes(ext)) return false;
  const size = item.totalBytes || item.fileSize || 0;
  if (c.minSizeKb > 0 && size > 0 && size < c.minSizeKb * 1024) return false;
  return true;
}

/**
 * Hand a browser download to the app. On success the browser download is
 * cancelled but stays in the browser's list, so it can be retried there if
 * the app's dialog is dismissed. If the app can't be reached the download
 * is left untouched. Returns true when the browser download was cancelled.
 */
async function takeOver(item, name) {
  const ok = await sendToApp({
    url: item.finalUrl || item.url,
    referrer: item.referrer || "",
    fileName: name ? name.split(/[\\/]/).pop() : null,
    totalBytes: item.totalBytes || item.fileSize || 0,
    mime: item.mime || null,
    storeId: item.cookieStoreId, // Firefox containers
  });
  if (!ok) return false;
  try {
    await B.downloads.cancel(item.id);
    return true;
  } catch {
    return false;
  }
}

if (!IS_FIREFOX && B.downloads?.onDeterminingFilename) {
  // Chromium: decide before the file name is final, so a captured download
  // never shows up half-started in the browser
  B.downloads.onDeterminingFilename.addListener((item, suggest) => {
    (async () => {
      let cancelled = false;
      try {
        await ensureFresh();
        if (shouldCapture(item, item.filename)) cancelled = await takeOver(item, item.filename);
      } catch {
        // fall through: the browser keeps the download
      } finally {
        // a cancelled download needs no name; anything else must get one
        if (!cancelled) {
          try { suggest(); } catch {}
        }
      }
    })();
    return true; // async suggest
  });
} else if (B.downloads?.onCreated) {
  B.downloads.onCreated.addListener(async (item) => {
    await ensureFresh();
    if (shouldCapture(item, item.filename)) await takeOver(item, item.filename);
  });
}

// ---------------------------------------------------------------- media sniffing
// The list lives in storage.session so it survives the service worker being
// stopped; Firefox versions without storage.session keep it in memory only.

const MEDIA_MIME = /^(video|audio)\//i;
const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|flv|wmv|mp3|m4a|aac|flac|wav|ogg|oga|opus)(\?|#|$)/i;
const SESSION = B.storage?.session;
let mediaReady = null;

function loadMedia() {
  if (!mediaReady) {
    mediaReady = (async () => {
      if (!SESSION) return;
      try {
        const d = await SESSION.get({ media: {} });
        for (const [tab, list] of Object.entries(d.media || {})) {
          const m = new Map();
          for (const it of list || []) if (it && it.url) m.set(it.url, it);
          if (m.size) state.media.set(Number(tab), m);
        }
      } catch {}
    })();
  }
  return mediaReady;
}

function saveMedia() {
  if (!SESSION) return;
  const o = {};
  for (const [tab, m] of state.media) o[tab] = [...m.values()];
  try { Promise.resolve(SESSION.set({ media: o })).catch(() => {}); } catch {}
}

/** Run a change to the media list after it was loaded, then persist it. */
function changeMedia(fn) {
  loadMedia().then(() => { if (fn() !== false) saveMedia(); });
}

async function mediaFor(tabId) {
  await loadMedia();
  return [...(state.media.get(tabId)?.values() ?? [])];
}

function noteMedia(tabId, url, mime, size) {
  if (tabId < 0 || !/^https?:/i.test(url)) return;
  if (/\.(m3u8|mpd)(\?|$)/i.test(url) || /mpegurl|dash\+xml/i.test(mime || "")) return; // streaming manifests
  if (size > 0 && size < 256 * 1024) return; // thumbnails / previews
  // strip byte-range query params some players add
  const clean = url.replace(/([?&])(range|bytes|rn|rbuf)=[^&]*/gi, "$1").replace(/[?&]$/, "");
  changeMedia(() => {
    let m = state.media.get(tabId);
    if (!m) state.media.set(tabId, (m = new Map()));
    if (m.has(clean)) return false;
    let name = "";
    try { name = decodeURIComponent(new URL(clean).pathname.split("/").pop() || ""); } catch {}
    m.set(clean, { url: clean, mime: mime || "", size: size || 0, name: name || "media" });
    if (m.size > 50) m.delete(m.keys().next().value);
    const action = B.action ?? B.browserAction;
    try { action.setBadgeText({ tabId, text: String(m.size) }); } catch {}
    try { action.setBadgeBackgroundColor({ color: "#5B9DFF" }); } catch {}
  });
}

if (B.webRequest?.onHeadersReceived) {
  B.webRequest.onHeadersReceived.addListener(
    (d) => {
      if (d.statusCode < 200 || d.statusCode >= 300) return;
      if (d.incognito) return;
      let mime = "", len = 0, range = "";
      for (const h of d.responseHeaders || []) {
        const n = h.name.toLowerCase();
        if (n === "content-type") mime = (h.value || "").split(";")[0].trim();
        else if (n === "content-length") len = parseInt(h.value, 10) || 0;
        else if (n === "content-range") range = h.value || "";
      }
      const total = range ? parseInt(range.split("/").pop(), 10) || len : len;
      if (MEDIA_MIME.test(mime) || MEDIA_EXT.test(d.url)) noteMedia(d.tabId, d.url, mime, total);
    },
    { urls: ["<all_urls>"], types: ["media", "object", "xmlhttprequest", "other"] },
    ["responseHeaders"]
  );
}

B.tabs?.onRemoved?.addListener((tabId) => changeMedia(() => state.media.delete(tabId)));
B.tabs?.onUpdated?.addListener((tabId, info) => {
  if (info.status === "loading" && info.url) {
    changeMedia(() => state.media.delete(tabId));
    const action = B.action ?? B.browserAction;
    try { action.setBadgeText({ tabId, text: "" }); } catch {}
  }
});

// ---------------------------------------------------------------- context menus

function buildMenus() {
  B.contextMenus.removeAll(() => {
    const add = (id, title, contexts) => B.contextMenus.create({ id, title, contexts });
    add("odm-link", "Download with Open Download Manager", ["link"]);
    add("odm-media", "Download media with Open Download Manager", ["video", "audio"]);
    add("odm-image", "Download image with Open Download Manager", ["image"]);
    add("odm-selection", "Download selected links with Open Download Manager", ["selection"]);
    add("odm-all", "Download all links on this page…", ["page"]);
  });
}
B.runtime.onInstalled.addListener(() => { buildMenus(); ping(); });
B.runtime.onStartup?.addListener(() => { buildMenus(); ping(); });

async function linksFromTab(tabId, selectionOnly) {
  try {
    const res = await B.tabs.sendMessage(tabId, { type: "odm-collect-links", selectionOnly });
    return (res && res.links) || [];
  } catch {
    return [];
  }
}

/** Offer the media a tab loaded (for streamed blob: players). */
async function sendTabMedia(tab, referrer) {
  const found = await mediaFor(tab?.id ?? -1);
  if (!found.length) {
    notifyNoMedia();
    return true;
  }
  return sendToApp({ urls: found.map((m) => m.url), referrer, storeId: await storeIdForTab(tab) });
}

B.contextMenus.onClicked.addListener(async (info, tab) => {
  const referrer = (tab && tab.url) || info.pageUrl || "";
  await ensureFresh();
  const storeId = await storeIdForTab(tab);
  let ok = true;
  if (info.menuItemId === "odm-link" && info.linkUrl) ok = await sendToApp({ url: info.linkUrl, referrer, storeId });
  else if ((info.menuItemId === "odm-media" || info.menuItemId === "odm-image") && info.srcUrl) {
    if (/^blob:/i.test(info.srcUrl)) ok = await sendTabMedia(tab, referrer);
    else ok = await sendToApp({ url: info.srcUrl, referrer, storeId });
  } else if (info.menuItemId === "odm-selection" || info.menuItemId === "odm-all") {
    const links = await linksFromTab(tab.id, info.menuItemId === "odm-selection");
    if (links.length) ok = await sendToApp({ urls: links, referrer, storeId });
  }
  if (!ok) notifyDown();
});

function notify(title, message) {
  try {
    B.notifications?.create?.({ type: "basic", iconUrl: "icons/icon-48.png", title, message });
  } catch {}
}

function notifyDown() {
  if (state.appRunning && !state.connected) {
    notify(
      "Open Download Manager isn't paired",
      "Copy the pairing key from the app (Settings → Browser integration) and paste it into this extension's popup."
    );
  } else {
    notify("Open Download Manager isn't running", "Start the app, then try again.");
  }
}

function notifyNoMedia() {
  notify("No media found on this page", "Play the video for a few seconds, then try again.");
}

// ---------------------------------------------------------------- messages

/** Messages from the extension's own pages (popup), not from content scripts. */
function fromExtensionPage(sender) {
  return !sender.tab && typeof sender.url === "string" && sender.url.startsWith(B.runtime.getURL(""));
}

async function tabById(tabId) {
  if (typeof tabId !== "number" || tabId < 0) return undefined;
  try { return await B.tabs.get(tabId); } catch { return undefined; }
}

B.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (sender.id && sender.id !== B.runtime.id) return false;
  const privileged = fromExtensionPage(sender);
  (async () => {
    switch (msg && msg.type) {
      case "odm-bypass":
        // modifier held on a click: let the next download through
        state.bypassUntil = Date.now() + 4000;
        sendResponse({ ok: true });
        break;
      case "odm-config":
        await ensureFresh();
        sendResponse({ connected: state.connected, capture: state.capture });
        break;
      case "odm-add": {
        const tab = privileged ? await tabById(msg.tabId) : sender.tab;
        sendResponse({
          ok: await sendToApp({
            url: msg.url,
            urls: msg.urls,
            referrer: msg.referrer || sender.tab?.url || "",
            fileName: msg.fileName,
            storeId: await storeIdForTab(tab),
          }),
        });
        break;
      }
      case "odm-add-media": {
        // the video button over a streamed (blob:) player
        const ok = await sendTabMedia(sender.tab, msg.referrer || sender.tab?.url || "");
        if (!ok) notifyDown();
        sendResponse({ ok });
        break;
      }
      case "odm-status": {
        await ping();
        // only the popup may ask about another tab
        const tabId = privileged ? (msg.tabId ?? -1) : (sender.tab?.id ?? -1);
        sendResponse({
          connected: state.connected,
          appRunning: state.appRunning,
          paired: state.paired,
          keyRejected: state.keyRejected,
          capture: state.capture,
          browser: browserName(),
          port: await port(),
          media: await mediaFor(tabId),
        });
        break;
      }
      case "odm-pause-capture":
        if (!privileged) { sendResponse({ error: "not allowed" }); break; }
        try {
          const info = await rpc("capture-pause", { minutes: Math.max(0, Math.min(1440, Number(msg.minutes) || 0)) });
          state.capture = info.capture || state.capture;
        } catch {}
        updateIcon();
        sendResponse({ capture: state.capture });
        break;
      case "odm-set-key": {
        // the pairing key from the app's settings (empty: forget it)
        if (!privileged) { sendResponse({ error: "not allowed" }); break; }
        const text = String(msg.key ?? "").trim();
        const hex = Pairing.parseKey(text);
        if (text && !hex) { sendResponse({ error: "That isn't a pairing key (64 letters and digits)." }); break; }
        await B.storage.local.set({ pairKey: hex || "" });
        sendResponse({ connected: await ping(), paired: state.paired, keyRejected: state.keyRejected });
        break;
      }
      case "odm-set-port": {
        if (!privileged) { sendResponse({ error: "not allowed" }); break; }
        const p = Number(msg.port);
        await B.storage.local.set({ port: Number.isInteger(p) && p > 0 && p < 65536 ? p : DEFAULT_PORT });
        sendResponse({ connected: await ping() });
        break;
      }
      default:
        sendResponse({});
    }
  })();
  return true;
});

// keep the app's "extension connected" indicator and capture config fresh
B.alarms?.create("odm-ping", { periodInMinutes: 1 });
B.alarms?.onAlarm.addListener((a) => { if (a.name === "odm-ping") ping(); });
loadMedia();
ping();
