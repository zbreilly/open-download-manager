// Open Download Manager — browser extension background logic.
// Works in Chromium MV3 (service worker) and Firefox MV2 (event page)
// via the chrome.* namespace. Talks to odm-serve on 127.0.0.1.

const DEFAULT_PORT = 8901;

async function getPort() {
  return new Promise((resolve) => {
    chrome.storage.local.get({ port: DEFAULT_PORT }, (d) => resolve(d.port || DEFAULT_PORT));
  });
}

async function getToken() {
  return new Promise((resolve) => {
    chrome.storage.local.get({ token: "" }, (d) => resolve(d.token || ""));
  });
}

async function odmAdd(details) {
  const port = await getPort();
  const body = JSON.stringify({
    url: details.url,
    referrer: details.referrer || "",
    fileName: details.fileName || "",
  });
  const token = await getToken();
  try {
    const r = await fetch(`http://127.0.0.1:${port}/api/extension/add`, {
      method: "POST",
      headers: { "content-type": "application/json", "x-odm-token": token },
      body,
    });
    return r.ok;
  } catch (e) {
    return false;
  }
}

async function serverUp() {
  const port = await getPort();
  const token = await getToken();
  try {
    const r = await fetch(`http://127.0.0.1:${port}/api/extension/status`, {
      method: "POST",
      headers: { "x-odm-token": token },
    });
    return r.ok;
  } catch (e) {
    return false;
  }
}

// ---- context menus ----
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({
      id: "odm-link",
      title: "Download link with Open Download Manager",
      contexts: ["link"],
    });
    chrome.contextMenus.create({
      id: "odm-page",
      title: "Add page URL to Open Download Manager",
      contexts: ["page"],
    });
    chrome.contextMenus.create({
      id: "odm-video",
      title: "Download video with Open Download Manager",
      contexts: ["video"],
    });
    chrome.contextMenus.create({
      id: "odm-audio",
      title: "Download audio with Open Download Manager",
      contexts: ["audio"],
    });
    chrome.contextMenus.create({
      id: "odm-image",
      title: "Download image with Open Download Manager",
      contexts: ["image"],
    });
  });
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const referrer = tab && tab.url ? tab.url : "";
  if (info.menuItemId === "odm-link" && info.linkUrl) {
    await odmAdd({ url: info.linkUrl, referrer });
  } else if (info.menuItemId === "odm-page" && info.pageUrl) {
    await odmAdd({ url: info.pageUrl, referrer });
  } else if (info.menuItemId === "odm-video" && (info.srcUrl || info.url)) {
    await odmAdd({ url: info.srcUrl || info.url, referrer });
  } else if (info.menuItemId === "odm-audio" && (info.srcUrl || info.url)) {
    await odmAdd({ url: info.srcUrl || info.url, referrer });
  } else if (info.menuItemId === "odm-image" && (info.srcUrl || info.url)) {
    await odmAdd({ url: info.srcUrl || info.url, referrer });
  }
});

// ---- keep the server's "extension connected" indicator alive ----
async function pingServer() {
  const port = await getPort();
  const token = await getToken();
  try {
    await fetch(`http://127.0.0.1:${port}/api/extension/status`, {
      method: "POST",
      headers: { "x-odm-token": token },
    });
  } catch (e) { /* server offline — retry on next alarm */ }
}
pingServer();
if (chrome.alarms) {
  chrome.alarms.create("odm-ping", { periodInMinutes: 1 });
  chrome.alarms.onAlarm.addListener((a) => {
    if (a.name === "odm-ping") pingServer();
  });
}
setInterval(pingServer, 30000); // event pages / popup-adjacent contexts

// ---- Alt-click capture (content scripts relay here) ----
chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (msg && msg.type === "odm-add" && msg.url) {
    odmAdd({ url: msg.url, referrer: msg.referrer || "" }).then((ok) =>
      sendResponse({ ok })
    );
    return true; // async response
  }
  if (msg && msg.type === "odm-status") {
    serverUp().then((up) => sendResponse({ up }));
    return true;
  }
});
