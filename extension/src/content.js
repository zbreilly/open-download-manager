// Open Download Manager — content script.
// * Reports the bypass modifier when you click a link, so that click goes
//   to the browser instead of the app.
// * Collects links for "Download all / selected links".
// * Shows a small "Download with ODM" button over playable videos.
"use strict";
(() => {
  const B = globalThis.browser ?? globalThis.chrome;
  if (window.__odmLoaded) return;
  window.__odmLoaded = true;

  let cfg = { connected: false, capture: { bypassKey: "alt", enabled: true } };
  const refresh = () => {
    try {
      B.runtime.sendMessage({ type: "odm-config" }).then((c) => { if (c) cfg = c; }).catch(() => {});
    } catch {}
  };
  refresh();
  setInterval(refresh, 60000);

  const held = (e) => {
    switch (cfg.capture?.bypassKey) {
      case "alt": return e.altKey;
      case "shift": return e.shiftKey;
      case "ctrl": return e.ctrlKey;
      default: return false;
    }
  };
  document.addEventListener("mousedown", (e) => {
    if (!held(e)) return;
    const a = e.target && e.target.closest && e.target.closest("a[href]");
    if (a) {
      try { B.runtime.sendMessage({ type: "odm-bypass" }); } catch {}
    }
  }, true);

  B.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
    if (msg?.type !== "odm-collect-links") return;
    const out = new Set();
    let anchors = [...document.querySelectorAll("a[href]")];
    if (msg.selectionOnly) {
      const sel = window.getSelection();
      anchors = sel && sel.rangeCount ? anchors.filter((a) => sel.containsNode(a, true)) : [];
      if (!anchors.length && sel) {
        // plain-text URLs in the selection
        for (const m of String(sel).matchAll(/https?:\/\/[^\s"'<>]+/g)) out.add(m[0]);
      }
    }
    for (const a of anchors) {
      if (/^https?:/i.test(a.href)) out.add(a.href.split("#")[0]);
    }
    sendResponse({ links: [...out].slice(0, 5000) });
    return true;
  });

  // ---------------- video overlay button ----------------
  const HOST_ID = "odm-video-button-host";
  let host = null, current = null, hideTimer = null;

  function ensureHost() {
    if (host) return host;
    host = document.createElement("div");
    host.id = HOST_ID;
    host.style.cssText = "position:absolute;z-index:2147483647;display:none;";
    const root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `
      <style>
        button{all:initial;display:flex;align-items:center;gap:8px;height:34px;padding:0 12px 0 10px;border-radius:9px;
          background:#5B9DFF;color:#0A1426;font:600 13px/1 system-ui,-apple-system,"Segoe UI",sans-serif;cursor:pointer;
          box-shadow:0 4px 14px rgba(0,0,0,.35)}
        button:hover{filter:brightness(1.08)}
        svg{width:18px;height:18px}
      </style>
      <button type="button" title="Download with Open Download Manager">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M20.21 14.2A8.5 8.5 0 1 0 14.2 20.21"/><path d="M12 7v8.5"/><path d="M8.6 12.1L12 15.5l3.4-3.4"/>
        </svg>Download</button>`;
    root.querySelector("button").addEventListener("click", (e) => {
      e.preventDefault();
      e.stopPropagation();
      // only a real click on a button the user can actually see: the page
      // owns this element's surroundings and could otherwise stretch or
      // hide it under something else to collect clicks
      if (!e.isTrusted || !current || !plainlyVisible(e)) return;
      const src = current.currentSrc || current.src || current.querySelector("source")?.src || "";
      // streamed (blob:) video: the background offers the media this tab
      // loaded, each with its own cookies, or says none was found
      const msg = /^https?:/i.test(src)
        ? { type: "odm-add", url: src, referrer: location.href }
        : { type: "odm-add-media", referrer: location.href };
      try { B.runtime.sendMessage(msg).catch(() => {}); } catch {}
    });
    host.addEventListener("mouseenter", () => clearTimeout(hideTimer));
    host.addEventListener("mouseleave", scheduleHide);
    document.documentElement.appendChild(host);
    return host;
  }

  /**
   * Is the button shown the way this script drew it: attached where it was
   * put, at its natural size, fully opaque, not transformed or clipped, and
   * the topmost thing at the click point?
   */
  function plainlyVisible(e) {
    if (!host || host.parentNode !== document.documentElement) return false;
    const r = host.getBoundingClientRect();
    if (r.width < 40 || r.width > 240 || r.height < 20 || r.height > 60) return false;
    const page = getComputedStyle(document.documentElement);
    if (page.visibility !== "visible" || Number(page.opacity) < 1) return false;
    const cs = getComputedStyle(host);
    if (cs.visibility !== "visible" || Number(cs.opacity) < 1) return false;
    if (cs.transform !== "none" || cs.filter !== "none" || cs.clipPath !== "none") return false;
    if (cs.mixBlendMode !== "normal") return false;
    if (cs.maskImage && cs.maskImage !== "none") return false;
    if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) return false;
    return document.elementFromPoint(e.clientX, e.clientY) === host;
  }

  function scheduleHide() {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => { if (host) host.style.display = "none"; current = null; }, 1200);
  }

  function showFor(v) {
    if (!cfg.connected) return;
    const r = v.getBoundingClientRect();
    if (r.width < 200 || r.height < 120) return;
    const h = ensureHost();
    current = v;
    clearTimeout(hideTimer);
    h.style.left = `${window.scrollX + r.right - 12 - 120}px`;
    h.style.top = `${window.scrollY + r.top + 12}px`;
    h.style.display = "block";
  }

  document.addEventListener("mouseover", (e) => {
    const v = e.target && e.target.closest ? e.target.closest("video") : null;
    if (v) showFor(v);
  }, true);
  document.addEventListener("mouseout", (e) => {
    const v = e.target && e.target.closest ? e.target.closest("video") : null;
    if (v) scheduleHide();
  }, true);
})();
