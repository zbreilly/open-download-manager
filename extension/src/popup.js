"use strict";
const B = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);
const fmt = (n) => !n ? "" : n < 1048576 ? `${(n / 1024).toFixed(0)} KB` : n < 1073741824 ? `${(n / 1048576).toFixed(1)} MB` : `${(n / 1073741824).toFixed(2)} GB`;
let tab = null;

async function render() {
  const st = await B.runtime.sendMessage({ type: "odm-status", tabId: tab?.id ?? -1 });
  $("port").value = st.port;
  const dot = $("dot");
  dot.className = "dot";
  if (st.link === "unpaired" || st.link === "wrongkey") {
    dot.classList.add("warn");
    $("status").textContent = st.link === "unpaired"
      ? "Not paired — paste the pairing code from the app below"
      : "The pairing code doesn't match the app — paste it again";
    $("capture-box").hidden = true;
    $("connection").open = true;
  } else if (!st.connected) {
    $("status").textContent = "App not running — start Open Download Manager";
    $("capture-box").hidden = true;
  } else {
    const c = st.capture;
    dot.classList.add(c.paused || !c.enabled ? "warn" : "ok");
    $("status").textContent = `Connected (${st.browser})`;
    $("capture-box").hidden = false;
    if (c.paused) {
      const mins = c.pausedUntil ? Math.max(1, Math.round((c.pausedUntil - Date.now()) / 60000)) : 10;
      $("capture-desc").textContent = `Paused — the browser handles downloads for ${mins} more min`;
      $("pause").textContent = "Resume now";
      $("pause").onclick = () => setPause(0);
    } else if (!c.enabled) {
      $("capture-desc").textContent = "Off in the app's settings";
      $("pause").hidden = true;
    } else {
      const key = { alt: navigator.platform.includes("Mac") ? "Option" : "Alt", shift: "Shift", ctrl: "Ctrl" }[c.bypassKey];
      $("capture-desc").textContent = key ? `Hold ${key} while clicking to skip` : "Matching downloads go to the app";
      $("pause").textContent = "Pause 10 min";
      $("pause").onclick = () => setPause(10);
    }
  }
  const media = st.media || [];
  $("media-box").hidden = !media.length;
  const ul = $("media");
  ul.textContent = "";
  for (const m of media) {
    const li = document.createElement("li");
    const n = document.createElement("span"); n.className = "n"; n.textContent = m.name; n.title = m.url;
    const s = document.createElement("span"); s.className = "s"; s.textContent = fmt(m.size);
    const b = document.createElement("button"); b.textContent = "Download";
    b.onclick = async () => { b.disabled = true; await B.runtime.sendMessage({ type: "odm-add", url: m.url, referrer: tab?.url || "", fileName: m.name, tabId: tab?.id }); b.textContent = "Sent"; };
    li.append(n, s, b);
    ul.append(li);
  }
}

async function setPause(minutes) {
  await B.runtime.sendMessage({ type: "odm-pause-capture", minutes });
  render();
}

$("add").onclick = async () => {
  const url = $("url").value.trim();
  if (!/^https?:\/\//i.test(url)) { $("url").focus(); return; }
  const r = await B.runtime.sendMessage({ type: "odm-add", url, referrer: tab?.url || "", tabId: tab?.id });
  if (r?.ok) { $("url").value = ""; window.close(); }
  else $("status").textContent = "Couldn't reach the app";
};
$("url").addEventListener("keydown", (e) => { if (e.key === "Enter") $("add").click(); });
$("pair").onclick = async () => {
  const r = await B.runtime.sendMessage({ type: "odm-set-pairing", code: $("code").value });
  if (r?.error) { $("pair-note").textContent = r.error; return; }
  $("code").value = "";
  $("pair-note").textContent = r?.connected ? "Paired." : r?.link === "wrongkey"
    ? "That code doesn't match the running app. Copy it again from the app."
    : "Saved. Start the app to finish.";
  render();
};
$("code").addEventListener("keydown", (e) => { if (e.key === "Enter") $("pair").click(); });
$("save").onclick = async () => { await B.runtime.sendMessage({ type: "odm-set-port", port: $("port").value }); render(); };

(async () => {
  try { [tab] = await B.tabs.query({ active: true, currentWindow: true }); } catch {}
  render();
})();
