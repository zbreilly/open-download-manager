"use strict";
const $ = (id) => document.getElementById(id);

async function getPort() {
  return new Promise((r) =>
    chrome.storage.local.get({ port: 8901 }, (d) => r(d.port || 8901))
  );
}

async function getToken() {
  return new Promise((r) =>
    chrome.storage.local.get({ token: "" }, (d) => r(d.token || ""))
  );
}

async function check() {
  const port = await getPort();
  $("port").value = port;
  const token = await getToken();
  $("token").value = token;
  chrome.storage.local.get({ autoCapture: true }, (d) => {
    $("autocap").checked = d.autoCapture !== false;
  });
  $("autocap").addEventListener("change", () => {
    chrome.storage.local.set({ autoCapture: $("autocap").checked });
  });
  try {
    const r = await fetch(`http://127.0.0.1:${port}/api/extension/status`, {
      method: "POST",
      headers: { "x-odm-token": token },
    });
    const d = await r.json();
    $("status").innerHTML = d.connected
      ? '<span class="ok">● Connected to Open Download Manager</span>'
      : '<span class="down">● Wrong token? Pair in Settings → Advanced</span>';
  } catch (e) {
    $("status").innerHTML =
      '<span class="down">● Not connected — start odm-serve</span>';
  }
}

$("add").addEventListener("click", async () => {
  const url = $("url").value.trim();
  if (!url) return;
  const port = await getPort();
  const token = await getToken();
  await fetch(`http://127.0.0.1:${port}/api/extension/add`, {
    method: "POST",
    headers: { "content-type": "application/json", "x-odm-token": token },
    body: JSON.stringify({ url }),
  });
  $("url").value = "";
  window.close();
});
$("save").addEventListener("click", async () => {
  const port = Number($("port").value) || 8901;
  await chrome.storage.local.set({ port, token: $("token").value.trim() });
  check();
});

check();
