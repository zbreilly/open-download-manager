// Open Download Manager — in-page media panel + Alt-click capture.
// Scans the page for video/audio/media links and offers a small
// collapsible floating panel listing them. Alt-click on a link also
// captures it.
"use strict";

const ODM_PANEL_ID = "odm-media-panel";
const esc = (v) => String(v).replace(/[&<>"']/g, (c) => ({
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const MEDIA_EXT_RE =
  /\.(mp4|m4v|webm|mkv|avi|mov|flv|wmv|mp3|m4a|flac|wav|ogg|opus|ts)(\?|$)/i;
// direct-download types intercepted automatically (configurable in popup)
const FILE_EXT_RE = new RegExp(
  "\\.(zip|rar|7z|tar|gz|bz2|xz|zst|iso|dmg|pkg|exe|msi|deb|rpm|apk|appimage|" +
  "mp4|m4v|webm|mkv|avi|mov|flv|wmv|mp3|m4a|flac|wav|ogg|opus|pdf|epub)(\\?|$)",
  "i");
let autoCapture = true;
chrome.storage.local.get({ autoCapture: true }, (d) => {
  autoCapture = d.autoCapture !== false;
});

const found = new Map(); // url -> {url, label}

function collectMedia() {
  const seen = new Set();
  const add = (url, label) => {
    if (!url || seen.has(url)) return;
    if (!/^https?:/.test(url)) return;
    seen.add(url);
    const name = decodeURIComponent(url.split("?")[0].split("/").pop() || "media");
    found.set(url, { url, label: label || name });
  };
  document.querySelectorAll("video, audio, source").forEach((el) => {
    if (el.src) add(el.src, el.tagName.toLowerCase());
    if (el.poster) seen.add(el.poster);
  });
  document.querySelectorAll("a[href]").forEach((a) => {
    if (MEDIA_EXT_RE.test(a.href)) add(a.href, a.textContent?.trim().slice(0, 40));
  });
}

function renderPanel() {
  collectMedia();
  let panel = document.getElementById(ODM_PANEL_ID);
  if (!found.size) {
    panel?.remove();
    return;
  }
  if (!panel) {
    panel = document.createElement("div");
    panel.id = ODM_PANEL_ID;
    panel.style.cssText = [
      "position:fixed", "right:16px", "bottom:16px", "z-index:2147483647",
      "width:280px", "max-height:40vh", "overflow:auto",
      "background:#14171c", "color:#e7e9ec", "border:1px solid #262c35",
      "border-radius:10px", "font:12px/1.4 system-ui, sans-serif",
      "box-shadow:0 8px 24px rgba(0,0,0,.4)", "padding:8px",
    ].join(";");
    document.documentElement.appendChild(panel);
  }
  const items = [...found.values()]
    .map(
      (m, i) => `
      <div style="display:flex;align-items:center;gap:6px;padding:4px 0;
                  border-bottom:1px solid #1f242b">
        <span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap"
              title="${esc(m.url)}">${esc(m.label || m.url)}</span>
        <button data-i="${i}" style="background:#5b9dff;color:#0a1426;border:0;
          border-radius:5px;padding:2px 8px;cursor:pointer;font-weight:600">Get</button>
      </div>`
    )
    .join("");
  panel.innerHTML =
    `<div style="display:flex;justify-content:space-between;align-items:center;
       padding-bottom:4px;border-bottom:1px solid #262c35">
       <b>Open Download Manager — ${found.size} media</b>
       <span id="${ODM_PANEL_ID}-min" style="cursor:pointer;color:#98a1ad">—</span>
     </div><div id="${ODM_PANEL_ID}-list">${items}</div>`;
  panel.querySelectorAll("button[data-i]").forEach((b) =>
    b.addEventListener("click", () => {
      const m = [...found.values()][Number(b.dataset.i)];
      chrome.runtime.sendMessage({ type: "odm-add", url: m.url, referrer: location.href });
      b.textContent = "✓";
      b.disabled = true;
    })
  );
  panel.querySelector(`#${ODM_PANEL_ID}-min`)?.addEventListener("click", () => {
    const list = document.getElementById(`${ODM_PANEL_ID}-list`);
    list.style.display = list.style.display === "none" ? "" : "none";
  });
}

// rescan on DOM changes ( SPA pages add media late )
let scanTimer = null;
const observer = new MutationObserver(() => {
  clearTimeout(scanTimer);
  scanTimer = setTimeout(renderPanel, 800);
});
observer.observe(document.documentElement, { childList: true, subtree: true });

// Click capture:
//  - Alt-click: capture ANY link (the manual bypass key)
//  - plain click on a direct-file link (zip/exe/mp4/pdf/...): captured
//    automatically, like the original app — toggleable from the popup
window.addEventListener(
  "click",
  (e) => {
    if (e.ctrlKey || e.metaKey || e.shiftKey) return;
    const a = e.target.closest && e.target.closest('a[href^="http"]');
    if (!a) return;
    const isFileLink = FILE_EXT_RE.test(a.pathname) || FILE_EXT_RE.test(a.href);
    if (!e.altKey && !(autoCapture && isFileLink)) return;
    e.preventDefault();
    e.stopPropagation();
    chrome.runtime.sendMessage({
      type: "odm-add",
      url: a.href,
      referrer: location.href,
    });
  },
  true
);

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", renderPanel);
} else {
  renderPanel();
}
