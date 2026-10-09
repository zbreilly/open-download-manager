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

const state = {
  connected: false,
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

async function call(path, body) {
  const p = await port();
  const init = {
    method: body === undefined ? "GET" : "POST",
    headers: { "x-odm-browser": browserName() },
  };
  if (body !== undefined) {
    init.headers["content-type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  const ctl = new AbortController();
  const t = setTimeout(() => ctl.abort(), 2500);
  try {
    const r = await fetch(`http://127.0.0.1:${p}${path}`, { ...init, signal: ctl.signal });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return await r.json();
  } finally {
    clearTimeout(t);
  }
}

async function ping() {
  try {
    const info = await call("/api/ping");
    state.connected = true;
    state.capture = info.capture || state.capture;
  } catch {
    state.connected = false;
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
    ? "Open Download Manager — app not running"
    : state.capture.paused
      ? "Open Download Manager — capture paused"
      : "Open Download Manager";
  try { action.setTitle({ title }); } catch {}
}

async function cookiesFor(url) {
  try {
    const list = await B.cookies.getAll({ url });
    return list.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return "";
  }
}

/** Send one or more URLs to the app. Returns true on success. */
async function sendToApp({ url, urls, referrer, fileName, totalBytes, mime, silent }) {
  const first = url || (urls && urls[0]);
  if (!first) return false;
  const cookies = await cookiesFor(first);
  try {
    await call("/api/add", {
      url: url || "",
      urls: urls || [],
      referrer: referrer || "",
      fileName: fileName || null,
      cookies,
      userAgent: navigator.userAgent,
      totalBytes: totalBytes > 0 ? totalBytes : null,
      mime: mime || null,
      silent: !!silent,
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
  const url = item.finalUrl || item.url || "";
  if (!/^https?:/i.test(url)) return false; // blob:, data:, file: stay in the browser
  const ext = extOf(name) || extOf(url);
  if (!ext || !c.extensions.includes(ext)) return false;
  const size = item.totalBytes || item.fileSize || 0;
  if (c.minSizeKb > 0 && size > 0 && size < c.minSizeKb * 1024) return false;
  return true;
}

async function takeOver(item, name) {
  const ok = await sendToApp({
    url: item.finalUrl || item.url,
    referrer: item.referrer || "",
    fileName: name ? name.split(/[\\/]/).pop() : null,
    totalBytes: item.totalBytes || item.fileSize || 0,
    mime: item.mime || null,
  });
  if (ok) {
    try { await B.downloads.cancel(item.id); } catch {}
    try { await B.downloads.erase({ id: item.id }); } catch {}
  }
  return ok;
}

if (!IS_FIREFOX && B.downloads?.onDeterminingFilename) {
  // Chromium: decide before the file name is final, so a captured download
  // never shows up half-started in the browser
  B.downloads.onDeterminingFilename.addListener((item, suggest) => {
    (async () => {
      await ensureFresh();
      if (shouldCapture(item, item.filename)) {
        const ok = await takeOver(item, item.filename);
        if (ok) return; // cancelled; suggest() not needed
      }
      suggest();
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

const MEDIA_MIME = /^(video|audio)\//i;
const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|flv|wmv|mp3|m4a|aac|flac|wav|ogg|oga|opus)(\?|#|$)/i;

function noteMedia(tabId, url, mime, size) {
  if (tabId < 0 || !/^https?:/i.test(url)) return;
  if (/\.(m3u8|mpd)(\?|$)/i.test(url) || /mpegurl|dash\+xml/i.test(mime || "")) return; // streaming manifests
  if (size > 0 && size < 256 * 1024) return; // thumbnails / previews
  let m = state.media.get(tabId);
  if (!m) state.media.set(tabId, (m = new Map()));
  // strip byte-range query params some players add
  const clean = url.replace(/([?&])(range|bytes|rn|rbuf)=[^&]*/gi, "$1").replace(/[?&]$/, "");
  if (m.has(clean)) return;
  let name = "";
  try { name = decodeURIComponent(new URL(clean).pathname.split("/").pop() || ""); } catch {}
  m.set(clean, { url: clean, mime: mime || "", size: size || 0, name: name || "media" });
  if (m.size > 50) m.delete(m.keys().next().value);
  const action = B.action ?? B.browserAction;
  try { action.setBadgeText({ tabId, text: String(m.size) }); } catch {}
  try { action.setBadgeBackgroundColor({ color: "#5B9DFF" }); } catch {}
}

if (B.webRequest?.onHeadersReceived) {
  B.webRequest.onHeadersReceived.addListener(
    (d) => {
      if (d.statusCode < 200 || d.statusCode >= 300) return;
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

B.tabs?.onRemoved?.addListener((tabId) => state.media.delete(tabId));
B.tabs?.onUpdated?.addListener((tabId, info) => {
  if (info.status === "loading" && info.url) {
    state.media.delete(tabId);
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

B.contextMenus.onClicked.addListener(async (info, tab) => {
  const referrer = (tab && tab.url) || info.pageUrl || "";
  await ensureFresh();
  let ok = true;
  if (info.menuItemId === "odm-link" && info.linkUrl) ok = await sendToApp({ url: info.linkUrl, referrer });
  else if ((info.menuItemId === "odm-media" || info.menuItemId === "odm-image") && info.srcUrl) {
    if (/^blob:/i.test(info.srcUrl)) {
      // streamed media: offer what the page actually loaded
      const found = [...(state.media.get(tab?.id ?? -1)?.values() ?? [])];
      ok = found.length ? await sendToApp({ urls: found.map((m) => m.url), referrer }) : false;
    } else ok = await sendToApp({ url: info.srcUrl, referrer });
  } else if (info.menuItemId === "odm-selection" || info.menuItemId === "odm-all") {
    const links = await linksFromTab(tab.id, info.menuItemId === "odm-selection");
    if (links.length) ok = await sendToApp({ urls: links, referrer });
  }
  if (!ok) notifyDown();
});

function notifyDown() {
  try {
    B.notifications?.create?.({
      type: "basic",
      iconUrl: "icons/icon-48.png",
      title: "Open Download Manager isn't running",
      message: "Start the app, then try again.",
    });
  } catch {}
}

// ---------------------------------------------------------------- messages

B.runtime.onMessage.addListener((msg, sender, sendResponse) => {
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
      case "odm-add":
        sendResponse({ ok: await sendToApp({ url: msg.url, urls: msg.urls, referrer: msg.referrer || sender.tab?.url || "", fileName: msg.fileName }) });
        break;
      case "odm-status": {
        await ping();
        const tabId = msg.tabId ?? sender.tab?.id ?? -1;
        sendResponse({
          connected: state.connected,
          capture: state.capture,
          browser: browserName(),
          port: await port(),
          media: [...(state.media.get(tabId)?.values() ?? [])],
        });
        break;
      }
      case "odm-pause-capture":
        try {
          const info = await call("/api/capture-pause", { minutes: msg.minutes });
          state.capture = info.capture || state.capture;
        } catch {}
        updateIcon();
        sendResponse({ capture: state.capture });
        break;
      case "odm-set-port":
        await B.storage.local.set({ port: Number(msg.port) || DEFAULT_PORT });
        sendResponse({ connected: await ping() });
        break;
      default:
        sendResponse({});
    }
  })();
  return true;
});

// keep the app's "extension connected" indicator and capture config fresh
B.alarms?.create("odm-ping", { periodInMinutes: 1 });
B.alarms?.onAlarm.addListener((a) => { if (a.name === "odm-ping") ping(); });
ping();
