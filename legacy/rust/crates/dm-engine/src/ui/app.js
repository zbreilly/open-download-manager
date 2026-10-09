/* Open Download Manager — main window client logic.
   Implements the design spec (download-manager-ui/HANDOFF.md §2–§3, §4)
   against the odm-serve REST API. All numerals render via --font-mono. */
"use strict";

const APP_NAME = "Open Download Manager"; // single branding point
const $ = (id) => document.getElementById(id);

/* ---- formatters (spec §2.3) ---- */
function fmtBytes(n) {
  if (n == null) return "—";
  if (n < 1024) return n + " B";
  const kb = n / 1024;
  if (kb < 1024) return kb.toFixed(1) + " KB";
  const mb = kb / 1024;
  if (mb < 1024) return mb.toFixed(1) + " MB";
  return (mb / 1024).toFixed(2) + " GB";
}
function fmtSpeed(bps) {
  return bps ? fmtBytes(bps) + "/s" : "—";
}
function fmtEta(secs) {
  if (secs == null || !isFinite(secs) || secs <= 0) return "—";
  if (secs > 86400) return "> 1 day";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  if (h > 0) return `${h}h ${String(m).padStart(2, "0")}m`;
  if (m > 0) return `${m}m ${String(s).padStart(2, "0")}s`;
  return `${s}s`;
}
function fmtPct(received, total) {
  if (!total) return "—";
  return Math.floor((received / total) * 100) + "%";
}
function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

/* ---- categories (server-driven; routed into folders on save) ---- */
let CATEGORIES = []; // [{name, folder, extensions[]}]
function categoryOf(fileName) {
  const ext = (fileName.split(".").pop() || "").toLowerCase();
  for (const c of CATEGORIES)
    if ((c.extensions || []).includes(ext)) return c.name;
  return "General";
}
async function loadCategories() {
  try {
    const d = await api("/api/categories");
    CATEGORIES = d.categories || [];
    const sel = $("add-category");
    if (sel && CATEGORIES.length) {
      sel.innerHTML = CATEGORIES
        .map((c) => `<option>${esc(c.name)}</option>`).join("");
    }
  } catch (e) { /* keep last */ }
}

/* ---- status model (spec §2.4) ---- */
const STATUS_LABEL = {
  queued: "Queued", connecting: "Connecting", downloading: "Downloading",
  paused: "Paused", assembling: "Finishing", complete: "Complete",
  error: "Failed",
};
const STATUS_COLOR = {
  downloading: "var(--accent)", connecting: "var(--accent)",
  assembling: "var(--accent)", paused: "var(--warning)",
  queued: "var(--text-muted)", error: "var(--danger)",
  complete: "var(--success)",
};

/* ---- state ---- */
const state = {
  jobs: [], queues: [],
  totals: { speedBps: 0, active: 0, paused: 0, queued: 0, failed: 0 },
  settings: {},
  filter: "all",
  search: "",
  selected: new Set(),
  focused: null,
  sortKey: "addedAt",
  sortDir: -1,
};

/* ---- toolbar model (user-editable, persisted as settings.toolbar) ---- */
const TOOLBAR_BUTTONS = [
  { id: "resume", label: "Resume", icon: "resume", act: () => actOnSelection("resume") },
  { id: "pause", label: "Pause", icon: "pause", act: () => actOnSelection("pause") },
  { id: "pause-all", label: "Pause all", icon: "pause-all", act: pauseAll },
  { id: "delete", label: "Delete", icon: "delete", act: () => actOnSelection("remove") },
  { id: "delete-completed", label: "Delete completed", icon: "delete-completed", act: deleteCompleted },
  { id: "start-queue", label: "Start queue", icon: "start-queue", act: startQueue },
  { id: "stop-queue", label: "Stop queue", icon: "stop-queue", act: stopQueue },
  { id: "scheduler", label: "Scheduler", icon: "scheduler",
    act: () => window.open("/scheduler", "odm-scheduler", "width=840,height=680") },
  { id: "speed-limit", label: "No limit", icon: "speed-limit", act: toggleLimit, menu: editLimit },
  { id: "settings", label: "Settings", icon: "options", iconOnly: true,
    act: () => window.open("/settings", "odm-settings", "width=980,height=720") },
];
const DEFAULT_TOOLBAR = ["resume", "pause", "pause-all", "delete", "delete-completed",
  "start-queue", "stop-queue", "scheduler", "speed-limit", "settings"];
function toolbarOrder() {
  const saved = (state.settings.toolbar || "").split(",").filter(
    (id) => TOOLBAR_BUTTONS.some((b) => b.id === id));
  // saved order is authoritative; empty/missing -> defaults
  return saved.length ? saved : DEFAULT_TOOLBAR.slice();
}

/* ---- api (token-authenticated; token injected same-origin) ---- */
function odmFetch(path, opts) {
  opts = opts || {};
  opts.headers = Object.assign({}, opts.headers, {
    "X-ODM-Token": window.ODM_TOKEN || "",
  });
  return fetch(path, opts);
}
async function api(path, opts) {
  const r = await odmFetch(path, opts);
  if (!r.ok) throw new Error(`${path}: ${r.status}`);
  return r.json();
}
function post(path) {
  return odmFetch(path, { method: "POST" }).then(refresh);
}
async function refresh() {
  try {
    const [jobs, totals, settings, ext] = await Promise.all([
      api("/api/jobs"),
      api("/api/totals").catch(() => null),
      api("/api/settings").catch(() => null),
      api("/api/extension").catch(() => null),
    ]);
    state.jobs = jobs.jobs || [];
    if (totals) state.totals = totals;
    if (settings) state.settings = settings;
    if (ext) renderExtensionStatus(ext.connected);
    render();
  } catch (e) {
    /* server unreachable — keep last frame */
  }
}

/* ---- actions ---- */
function actOnSelection(action) {
  for (const id of state.selected) {
    if (action === "resume") post(`/api/resume?id=${id}`);
    else if (action === "pause") post(`/api/pause?id=${id}`);
    else if (action === "remove") post(`/api/remove?id=${id}`);
  }
  if (action === "resume") post("/api/run?max=4");
}
function pauseAll() {
  state.jobs
    .filter((j) => j.status === "downloading")
    .forEach((j) => post(`/api/pause?id=${j.id}`));
}
function deleteCompleted() {
  state.jobs
    .filter((j) => j.status === "complete")
    .forEach((j) => post(`/api/remove?id=${j.id}`));
}
function startQueue() {
  post("/api/run?max=4");
}
function stopQueue() {
  pauseAll();
}
async function toggleLimit() {
  const cur = Number(state.settings.globalLimit || 0);
  const next = cur > 0 ? 0 : 25;
  state.settings.globalLimit = String(next);
  await odmFetch(`/api/settings?globalLimit=${next}`, { method: "POST" });
  renderToolbar();
}
function editLimit(e) {
  e.preventDefault();
  const v = prompt("Global speed limit (MB/s, 0 = off):", "25");
  if (v == null) return;
  const next = Math.max(0, Number(v) || 0);
  state.settings.globalLimit = String(next);
  odmFetch(`/api/settings?globalLimit=${next}`, { method: "POST" })
    .then(() => renderToolbar());
}

/* ---- toolbar (spec §3.3 + user customization) ---- */
function renderToolbar() {
  const bar = $("toolbar");
  const order = toolbarOrder();
  let html = `<button class="btn primary" id="btn-add">
    <svg class="ic"><use href="/icons/sprite.svg#add-url"/></svg>Add URL</button>
    <span class="vdiv"></span>`;
  for (const id of order) {
    const b = TOOLBAR_BUTTONS.find((x) => x.id === id);
    if (!b) continue;
    let label = b.label;
    if (b.id === "speed-limit") {
      const v = Number(state.settings.globalLimit || 0);
      label = v > 0 ? `Limit ${v} MB/s` : "No limit";
    }
    const pressed = b.id === "speed-limit" && Number(state.settings.globalLimit || 0) > 0;
    html += `<button class="btn${b.iconOnly ? " icon" : ""}" id="tb-${b.id}"
      aria-pressed="${pressed}" ${b.iconOnly ? `aria-label="${b.label}" title="${b.label}"` : ""}
      ${b.menu ? 'data-menu="1"' : ""}>
      <svg class="ic"><use href="/icons/sprite.svg#${b.icon}"/></svg>${b.iconOnly ? "" : label}</button>`;
  }
  bar.innerHTML = html;
  $("btn-add").addEventListener("click", openAddDialog);
  for (const id of order) {
    const b = TOOLBAR_BUTTONS.find((x) => x.id === id);
    const el = $(`tb-${b.id}`);
    if (!el) continue;
    el.addEventListener("click", b.act);
    if (b.menu) el.addEventListener("contextmenu", b.menu);
  }
}

/* toolbar editor: right-click the toolbar to add/remove buttons */
function openToolbarEditor() {
  document.querySelectorAll(".ctxmenu").forEach((m) => m.remove());
  const m = document.createElement("div");
  m.className = "ctxmenu toolbar-editor";
  const order = toolbarOrder();
  const all = DEFAULT_TOOLBAR;
  m.innerHTML =
    `<div class="ctx-title">Customize toolbar</div>` +
    all.map((id) => {
      const b = TOOLBAR_BUTTONS.find((x) => x.id === id);
      const checked = order.includes(id) ? "checked" : "";
      return `<label class="ctx-check"><input type="checkbox" data-id="${id}" ${checked}>
        <svg class="ic"><use href="/icons/sprite.svg#${b.icon}"/></svg>${b.label}</label>`;
    }).join("") +
    `<div class="ctx-actions">
       <button id="tb-reset">Reset</button>
       <button id="tb-save" class="primary">Save</button></div>`;
  document.body.appendChild(m);
  const rect = $("toolbar").getBoundingClientRect();
  m.style.left = Math.min(rect.left + 8, innerWidth - 240) + "px";
  m.style.top = rect.bottom + 6 + "px";
  m.addEventListener("click", (e) => e.stopPropagation());
  m.querySelector("#tb-save").addEventListener("click", () => {
    const ids = [...m.querySelectorAll("input:checked")].map((i) => i.dataset.id);
    state.settings.toolbar = ids.join(",");
    odmFetch(`/api/settings?toolbar=${ids.join(",")}`, { method: "POST" })
      .then(renderToolbar);
    m.remove();
  });
  m.querySelector("#tb-reset").addEventListener("click", () => {
    odmFetch(`/api/settings?toolbar=${DEFAULT_TOOLBAR.join(",")}`, { method: "POST" })
      .then(() => { state.settings.toolbar = DEFAULT_TOOLBAR.join(","); renderToolbar(); });
    m.remove();
  });
  setTimeout(() =>
    document.addEventListener("click", () => m.remove(), { once: true }), 0);
}

/* ---- add dialog (spec §4) ---- */
let probeTimer = null;
function openAddDialog() {
  $("add-dialog").showModal();
  loadCategories();
  loadRecent();
  loadQueuesIntoDialog();
  onAddUrlInput();
}
function onAddUrlInput() {
  clearTimeout(probeTimer);
  probeTimer = setTimeout(async () => {
    const url = $("add-url").value.trim();
    const card = $("probe-card");
    if (!/^https?:\/\//.test(url)) { card.hidden = true; return; }
    card.hidden = false;
    $("probe-size").textContent = "Checking link…";
    $("probe-mime").textContent = "";
    $("probe-chips").innerHTML = "";
    try {
      const p = await api(`/api/probe?url=${encodeURIComponent(url)}`);
      $("probe-size").textContent = p.totalBytes ? fmtBytes(p.totalBytes) : "unknown size";
      $("probe-mime").textContent = p.contentType || "";
      $("probe-chips").innerHTML = p.resumable
        ? `<span class="chip ok">Resumable</span><span class="chip ok">Byte ranges</span><span class="chip">${esc(p.protocol)}</span>`
        : `<span class="chip warn">Server doesn't support resume — 1 connection</span>`;
      setConnectionsDisabled(!p.resumable);
      if (p.fileName) $("add-name").value = p.fileName;
      $("add-category").value = categoryOf(p.fileName || url);
    } catch (e) {
      $("probe-size").textContent = "Probe failed";
    }
  }, 300);
}
function setConnectionsDisabled(disabled) {
  document.querySelectorAll("#add-parts button").forEach((b) => {
    b.disabled = disabled && b.textContent.trim() !== "1";
    if (disabled) b.setAttribute("aria-pressed", b.textContent.trim() === "1");
  });
}
async function loadRecent() {
  try {
    const d = await api("/api/recent");
    $("recent-chips").innerHTML = (d.folders || [])
      .map((f) => `<span class="chip" data-dir="${esc(f)}">${esc(f)}</span>`)
      .join("");
    $("recent-chips").querySelectorAll(".chip").forEach((c) =>
      c.addEventListener("click", () => { $("add-out").value = c.dataset.dir; }));
  } catch (e) { /* empty */ }
}
async function loadQueuesIntoDialog() {
  try {
    const d = await api("/api/queues");
    state.queues = d.queues || [];
    $("add-queue").innerHTML = state.queues
      .map((q) => `<option>${esc(q.name || q.id)}</option>`).join("");
  } catch (e) { /* keep */ }
}
async function submitAdd(e) {
  if (e.submitter?.value !== "start") return;
  const url = $("add-url").value.trim();
  let out = $("add-out").value.trim();
  const name = $("add-name").value.trim();
  if (name && out) out = out.replace(/\/?$/, "/") + name;
  if (!url || !out) return;
  const parts = document.querySelector("#add-parts button[aria-pressed=true]")?.textContent.trim() || "4";
  const params = new URLSearchParams({
    url, out,
    authUser: $("adv-user").value,
    authPass: $("adv-pass").value,
    referrer: $("adv-ref").value,
    cookie: $("adv-cookie").value,
    headers: $("adv-headers").value,
  });
  // file-exists choice per spec: overwrite / rename / skip
  const ex = await odmFetch(`/api/exists?path=${encodeURIComponent(out)}`)
    .then((r) => r.json()).catch(() => ({ exists: false }));
  let finalOut = out;
  if (ex.exists) {
    const c = prompt("File exists: [o]verwrite, [r]ename, [s]kip?", "o");
    if (c == null || c.toLowerCase() === "s") return;
    if (c.toLowerCase() === "r") {
      const m = out.match(/^(.*?)(\.[^.]*)?$/);
      finalOut = `${m[1]} (1)${m[2] || ""}`;
    }
  }
  params.set("out", finalOut);
  await odmFetch(`/api/add?${params}`, { method: "POST" });
  post(`/api/run?max=4&parts=${parts}`);
}

/* ---- context menu (spec §3.4) ---- */
function ctxMenu(x, y, items) {
  document.querySelectorAll(".ctxmenu").forEach((m) => m.remove());
  const m = document.createElement("div");
  m.className = "ctxmenu";
  m.innerHTML = items.map((i) =>
    i ? `<button class="${i.danger ? "danger" : ""}" data-act="${i.act}" data-id="${i.id}">
      ${i.icon ? `<svg class="ic"><use href="/icons/sprite.svg#${i.icon}"/></svg>` : ""}${esc(i.label)}</button>`
    : `<hr>`).join("");
  document.body.appendChild(m);
  m.style.left = Math.min(x, innerWidth - 200) + "px";
  m.style.top = Math.min(y, innerHeight - m.offsetHeight - 8) + "px";
  m.addEventListener("click", (e) => {
    const act = e.target.dataset?.act, id = e.target.dataset?.id;
    m.remove();
    if (!act) return;
    if (act === "pause") post(`/api/pause?id=${id}`);
    if (act === "resume") post(`/api/resume?id=${id}`).then(() => post("/api/run?max=4"));
    if (act === "remove") post(`/api/remove?id=${id}`);
    if (act === "removefiles") post(`/api/remove?id=${id}&files=1`);
    if (act === "openfolder") {
      const j = state.jobs.find((x) => x.id == id);
      if (j) post(`/api/open?path=${encodeURIComponent(j.out)}&folder=1`);
    }
    if (act === "copylink") {
      const j = state.jobs.find((x) => x.id == id);
      if (j && navigator.clipboard) navigator.clipboard.writeText(j.url);
    }
    if (act === "progress") window.open(`/progress?id=${id}`, "odm-progress-" + id, "width=780,height=700");
  });
  setTimeout(() => document.addEventListener("click", () => m.remove(), { once: true }), 0);
}

/* ---- filtering + rendering ---- */
function matchesFilters(j) {
  const q = state.search;
  if (q && !`${j.fileName} ${j.url} ${j.out}`.toLowerCase().includes(q)) return false;
  switch (state.filter) {
    case "active": return ["downloading", "connecting", "assembling"].includes(j.status);
    case "finished": return j.status === "complete";
    case "unfinished": return j.status !== "complete";
    default:
      if (state.filter.startsWith("cat:"))
        return categoryOf(j.fileName) === state.filter.slice(4);
      return true;
  }
}

function fileBadge(name) {
  const ext = (name.split(".").pop() || "?").toUpperCase().slice(0, 4);
  return `<span class="badge">${esc(ext)}</span>`;
}

function render() {
  // counts
  const counts = {
    all: state.jobs.length,
    active: state.jobs.filter((j) => ["downloading", "connecting", "assembling"].includes(j.status)).length,
    unfinished: state.jobs.filter((j) => j.status !== "complete").length,
    finished: state.jobs.filter((j) => j.status === "complete").length,
  };
  for (const [k, v] of Object.entries(counts)) {
    const el = $(`c-${k}`);
    if (el) el.textContent = v;
  }
  for (const cat of Object.keys(CATEGORIES)) {
    const el = $(`c-${cat.toLowerCase()}`);
    if (el) el.textContent = state.jobs.filter((j) => categoryOf(j.fileName) === cat).length;
  }
  renderQueuesNav();

  const totalSpeed = state.totals.speedBps || 0;
  $("total-speed").textContent = totalSpeed ? fmtSpeed(totalSpeed) : "—";
  $("sb-counts").textContent =
    `${state.totals.active || 0} active · ${state.totals.paused || 0} paused · ` +
    `${state.totals.queued || 0} queued · ${state.totals.failed || 0} failed`;
  $("sb-speed").textContent = `↓ ${fmtSpeed(totalSpeed)}`;
  const limit = Number(state.settings.globalLimit || 0);
  $("sb-limit").textContent = limit > 0 ? `Speed limit: ${limit} MB/s` : "Speed limit: off";

  // sorting
  const keyMap = { name: "fileName", size: "totalBytes", progress: "receivedBytes",
    speed: "speedBps", eta: "etaSecs", status: "status", added: "addedAt" };
  const val = (j) => {
    const k = keyMap[state.sortKey] || "addedAt";
    const v = j[k];
    return v == null ? 0 : v;
  };
  const visible = state.jobs.filter(matchesFilters)
    .sort((a, b) => {
      const va = val(a), vb = val(b);
      return (va < vb ? -1 : va > vb ? 1 : 0) * state.sortDir;
    });

  const rows = $("rows");
  $("empty").hidden = visible.length > 0;
  rows.innerHTML = visible.map((j) => {
    const sel = state.selected.has(j.id) ? " selected" : "";
    const pct = fmtPct(j.receivedBytes, j.totalBytes);
    const pctFill = j.status === "paused" ? "var(--warning)"
      : j.status === "error" ? "var(--danger)"
      : j.status === "complete" ? "var(--success)" : "var(--accent)";
    const width = j.totalBytes ? (100 * j.receivedBytes / j.totalBytes).toFixed(1) : 0;
    const color = STATUS_COLOR[j.status] || "var(--text-muted)";
    const label = STATUS_LABEL[j.status] || j.status;
    return `<tr class="row${sel}" data-id="${j.id}">
      <td><span class="file-cell">${fileBadge(j.fileName)}<span class="file-name">${esc(j.fileName)}</span></span></td>
      <td class="mono">${fmtBytes(j.totalBytes)}</td>
      <td><span class="pbar-cell"><span class="pbar"><div style="width:${width}%;background:${pctFill}"></div></span><span class="pct">${pct}</span></span></td>
      <td class="right">${fmtSpeed(j.speedBps)}</td>
      <td class="right">${fmtEta(j.etaSecs)}</td>
      <td><span class="status-pill" style="--sd:${color}"><span class="sdot"></span>${label}</span></td>
      <td class="muted">—</td>
      <td class="muted">${esc(j.addedAt || "—")}</td>
    </tr>`;
  }).join("");

  if (!rows.dataset.sortable) {
    rows.dataset.sortable = "1";
    rows.parentElement.querySelectorAll("th").forEach((th, i) => {
      const keys = ["name", "size", "progress", "speed", "eta", "status", "queue", "added"];
      th.style.cursor = "pointer";
      th.addEventListener("click", () => {
        if (state.sortKey === keys[i]) state.sortDir *= -1;
        else { state.sortKey = keys[i]; state.sortDir = -1; }
        render();
      });
    });
  }
  for (const tr of rows.querySelectorAll("tr.row")) {
    const id = Number(tr.dataset.id);
    tr.addEventListener("click", (e) => {
      if (e.ctrlKey || e.metaKey) {
        state.selected.has(id) ? state.selected.delete(id) : state.selected.add(id);
      } else if (e.shiftKey && state.selected.size) {
        const ids = visible.map((x) => x.id);
        const last = [...state.selected].pop();
        const a = ids.indexOf(last), b = ids.indexOf(id);
        for (const x of ids.slice(Math.min(a, b), Math.max(a, b) + 1)) state.selected.add(x);
      } else {
        state.selected.clear();
        state.selected.add(id);
        state.focused = id;
      }
      render();
    });
  }

  const hasSel = state.selected.size > 0;
  setToolbarEnabled("resume", hasSel);
  setToolbarEnabled("pause", hasSel);
  setToolbarEnabled("delete", hasSel);

  renderDetails(visible);
}

function setToolbarEnabled(id, enabled) {
  const el = $(`tb-${id}`);
  if (el) el.disabled = !enabled;
}

function renderQueuesNav() {
  const el = $("nav-queues-list");
  if (!el) return;
  if (!state.queues.length) {
    odmFetch("/api/queues").then((r) => r.json())
      .then((d) => { state.queues = d.queues || []; renderQueuesNav(); })
      .catch(() => {});
    return;
  }
  el.innerHTML = state.queues.map((q) =>
    `<button class="nav-item" onclick="window.open('/scheduler','odm-scheduler','width=840,height=680')">
      <svg class="ic"><use href="/icons/sprite.svg#cat-queues"/></svg>${esc(q.name || q.id)}</button>`).join("");
}

function renderExtensionStatus(connected) {
  const sb = $("sb-extension");
  if (sb) sb.innerHTML = connected
    ? '<span class="dot-on"></span> Browser extension connected'
    : '<span class="dot-off"></span> Browser extension not detected';
}

function renderDetails(visible) {
  const panel = $("details");
  const j = state.jobs.find((x) => x.id === state.focused) || visible[0] || state.jobs[0];
  if (!j) { panel.hidden = true; return; }
  panel.hidden = false;
  const color = STATUS_COLOR[j.status], label = STATUS_LABEL[j.status] || j.status;
  const ext = (j.fileName.split(".").pop() || "?").toUpperCase().slice(0, 4);
  const action =
    j.status === "downloading"
      ? `<button class="btn" onclick="post('/api/pause?id=${j.id}')">Pause</button>`
    : j.status === "paused" || j.status === "queued"
      ? `<button class="btn" onclick="post('/api/resume?id=${j.id}');post('/api/run?max=4')">Resume</button>`
    : j.status === "complete"
      ? `<button class="btn" onclick="post('/api/open?path=${encodeURIComponent(j.out)}')">Open file</button>
         <button class="btn" onclick="post('/api/open?path=${encodeURIComponent(j.out)}&folder=1')">Open folder</button>`
    : `<button class="btn" onclick="post('/api/resume?id=${j.id}');post('/api/run?max=4')">Retry</button>`;
  const hist = j.speedHistory || [];
  const peak = Math.max(0, ...hist, 0);
  const pts = hist.length > 1
    ? hist.map((v, i) => `${(i / (hist.length - 1)) * 220},${64 - (v / Math.max(peak, 1)) * 60}`).join(" ")
    : "";
  panel.innerHTML = `
    <div class="top">
      <span class="badge lg">${esc(ext)}</span>
      <div style="min-width:0;flex:1">
        <div class="name">${esc(j.fileName)}</div>
        <div class="url">${esc(j.url)}</div>
      </div>
      <span class="status-pill" style="--sd:${color}"><span class="sdot"></span>${label}</span>
      ${action}
    </div>
    <div class="cols"><dl>
      <dt>Downloaded</dt><dd>${fmtBytes(j.receivedBytes)} of ${fmtBytes(j.totalBytes)}</dd>
      <dt>Speed</dt><dd>${fmtSpeed(j.speedBps)}</dd>
      <dt>Time left</dt><dd>${fmtEta(j.etaSecs)}</dd>
      <dt>Connections</dt><dd id="detail-conns">${j.status === "downloading" ? "…" : "—"}</dd>
      <dt>Resume</dt><dd>Supported (byte ranges)</dd>
      <dt>Save to</dt><dd>${esc(j.out)}</dd>
    </dl>
    <div style="flex:1;min-width:220px">
      <div class="muted sect-label">Speed (60 s)</div>
      <svg viewBox="0 0 220 64" style="width:220px;height:64px;background:var(--surface-2);border-radius:6px">
        <polyline fill="none" stroke="var(--accent)" stroke-width="1.8" points="${pts}"></polyline>
      </svg>
      <div class="muted" style="font-size:var(--fs-xs)">peak ${fmtBytes(peak)}/s</div>
    </div></div>
    <div id="segmap-mini" class="segmap" style="display:flex;gap:3px;height:22px;margin-top:12px"></div>`;
  if (["downloading", "connecting"].includes(j.status)) {
    odmFetch(`/api/segments?id=${j.id}`).then((r) => r.json()).then((d) => {
      const el2 = document.getElementById("segmap-mini");
      if (el2 && d.segments) {
        el2.innerHTML = d.segments.map((sg) => {
          const size = sg.end - sg.start + 1;
          const f = Math.min(100, (100 * sg.received) / size);
          return `<div style="flex:1;background:var(--track);border-radius:4px;overflow:hidden">
            <div style="height:100%;width:${f}%;background:var(--accent)"></div></div>`;
        }).join("");
      }
    }).catch(() => {});
  }
}

/* ---- theme ---- */
function applyTheme(t) {
  document.documentElement.dataset.theme =
    t === "system"
      ? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light")
      : t;
}

/* ---- wiring (single listeners) ---- */
window.ui = { addFromEmpty: openAddDialog };
$("btn-pause-all-legacy")?.remove(); // guard against stale markup
document.querySelector(".toolbar")?.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  openToolbarEditor();
});
$("search").addEventListener("input", (e) => {
  clearTimeout(window.__searchTimer);
  window.__searchTimer = setTimeout(() => {
    state.search = e.target.value.toLowerCase();
    render();
  }, 120);
});
document.querySelectorAll(".nav-item[data-filter]").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll(".nav-item").forEach((x) => x.classList.remove("selected"));
    b.classList.add("selected");
    state.filter = b.dataset.filter;
    render();
  }));
$("rows").addEventListener("dblclick", (e) => {
  const tr = e.target.closest("tr.row");
  if (!tr) return;
  window.open(`/progress?id=${tr.dataset.id}`, "odm-progress-" + tr.dataset.id,
    "width=780,height=700");
});
$("rows").addEventListener("contextmenu", (e) => {
  const tr = e.target.closest("tr.row");
  if (!tr) return;
  e.preventDefault();
  const id = Number(tr.dataset.id);
  if (!state.selected.has(id)) { state.selected.clear(); state.selected.add(id); render(); }
  ctxMenu(e.clientX, e.clientY, [
    { label: "Pause", act: "pause", id, icon: "pause" },
    { label: "Resume", act: "resume", id, icon: "resume" },
    null,
    { label: "Open folder", act: "openfolder", id, icon: "open-folder" },
    { label: "Copy link", act: "copylink", id, icon: "copy-link" },
    { label: "Progress window", act: "progress", id, icon: "download" },
    null,
    { label: "Remove", act: "remove", id, icon: "delete" },
    { label: "Remove + delete file", act: "removefiles", id, icon: "delete", danger: true },
  ]);
});
$("add-form").addEventListener("submit", submitAdd);
$("add-url").addEventListener("input", onAddUrlInput);
$("add-category")?.addEventListener("change", () => {
  const cat = CATEGORIES.find((c) => c.name === $("add-category").value);
  const cur = $("add-out").value.trim();
  // only auto-rewrite when the field is empty or points at the download root
  if (cat && cat.folder && (!cur || cur.split("/").pop() === "Downloads")) {
    $("add-out").value = cur.replace(/Downloads$/, "Downloads/" + cat.folder);
  }
});
$("adv-toggle").addEventListener("click", () => {
  const adv = $("adv");
  adv.hidden = !adv.hidden;
});
// segmented connections control (spec §4.6)
document.querySelectorAll("#add-parts button").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll("#add-parts button").forEach((x) =>
      x.setAttribute("aria-pressed", "false"));
    b.setAttribute("aria-pressed", "true");
  }));
document.addEventListener("keydown", (e) => {
  const typing = ["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement?.tagName);
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    $("search").focus();
    return;
  }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "n") {
    e.preventDefault();
    openAddDialog();
    return;
  }
  if (typing) return;
  if (e.key === " " && state.selected.size) {
    e.preventDefault();
    actOnSelection("pause");
  }
  if (e.key === "Delete" && state.selected.size) {
    actOnSelection("remove");
    state.selected.clear();
    render();
  }
  if (e.key === "Enter" && state.focused != null) {
    window.open(`/progress?id=${state.focused}`, "odm-progress-" + state.focused,
      "width=780,height=700");
  }
});

/* ---- boot ---- */
renderToolbar();
applyTheme("dark");
odmFetch("/api/settings").then((r) => r.json())
  .then((s) => { state.settings = s; applyTheme(s.theme || "dark"); renderToolbar(); })
  .catch(() => {});
refresh();
setInterval(refresh, 500);
