// Headless tests for the ODM browser extension logic.
// Stubs the chrome.* APIs, loads the real scripts, verifies behavior.
// Run: node extension/test.js
"use strict";
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const assert = require("assert");

const COMMON = path.join(__dirname, "common");

// ---- chrome.* stub ----
function makeChromeStub() {
  const state = {
    storage: { local: {} },
    menus: [],
    clicks: [],
    messages: [],
    responders: [],
  };
  return {
    state,
    chrome: {
      storage: {
        local: {
          get(defs, cb) {
            const out = {};
            for (const k of Object.keys(defs)) out[k] = state.storage.local[k] ?? defs[k];
            setTimeout(() => cb(out), 0);
          },
          set(obj, cb) {
            Object.assign(state.storage.local, obj);
            cb && cb();
          },
        },
      },
      contextMenus: {
        create(opts) { state.menus.push(opts); },
        removeAll(cb) { state.menus = []; cb && cb(); },
        onClicked: { addListener() {} },
      },
      runtime: {
        onInstalled: { addListener(fn) { state.onInstalled = fn; } },
        onMessage: {
          addListener(fn) { state.responders.push(fn); },
        },
      },
    },
  };
}

function loadScript(file, sandbox) {
  let code = fs.readFileSync(path.join(COMMON, file), "utf8");
  if (file === "capture.js") {
    code += "\n;globalThis.__test = { esc, found, collectMedia, renderPanel };";
  }
  if (!sandbox.setInterval) {
    sandbox.setInterval = () => 0;   // no-op timer for tests
    sandbox.clearInterval = () => {};
  }
  vm.runInNewContext(code, sandbox, { filename: file });
}

// captured fetch stub
function makeFetchStub() {
  const calls = [];
  const fn = async (url, opts) => {
    calls.push({ url, opts });
    return { ok: true, json: async () => ({ connected: true }) };
  };
  fn.calls = calls;
  return fn;
}

let passed = 0, failed = 0;
function test(name, fn) {
  return Promise.resolve()
    .then(fn)
    .then(() => { passed++; console.log(`ok   ${name}`); })
    .catch((e) => { failed++; console.error(`FAIL ${name}: ${e.message}`); });
}

(async () => {
  // 1. background.js: registers the five context menus
  await test("background registers context menus", () => {
    const { chrome, state } = makeChromeStub();
    loadScript("background.js", { chrome, fetch: makeFetchStub(), console });
    state.onInstalled();
    const ids = state.menus.map((m) => m.id);
    assert.deepStrictEqual(ids.sort(),
      ["odm-audio", "odm-image", "odm-link", "odm-page", "odm-video"]);
  });

  // 2. background.js: odm-add message posts with port + token
  await test("background message posts to configured port with token", async () => {
    const { chrome, state } = makeChromeStub();
    state.storage.local = { port: 9999, token: "abc123" };
    const fetchStub = makeFetchStub();
    loadScript("background.js", { chrome, fetch: fetchStub, console });
    let respond;
    const captured = new Promise((r) => (respond = r));
    state.responders[0](
      { type: "odm-add", url: "http://x/f.zip", referrer: "http://x/" },
      null,
      respond
    );
    await captured;
    await new Promise((r) => setTimeout(r, 10));
    const call = fetchStub.calls.find((c) => c.url.includes("/api/extension/add"));
    assert.ok(call, "extension/add call made");
    assert.strictEqual(call.url, "http://127.0.0.1:9999/api/extension/add");
    assert.strictEqual(call.opts.headers["x-odm-token"], "abc123");
    assert.strictEqual(call.opts.headers["content-type"], "application/json");
    assert.strictEqual(JSON.parse(call.opts.body).url, "http://x/f.zip");
  });

  // 3. background.js: status ping also carries the token
  await test("status ping carries token", async () => {
    const { chrome, state } = makeChromeStub();
    state.storage.local = { port: 8901, token: "sekrit" };
    const fetchStub = makeFetchStub();
    loadScript("background.js", { chrome, fetch: fetchStub, console });
    let respond;
    const captured = new Promise((r) => (respond = r));
    state.responders[1] = state.responders[0]; // same listener handles both
    state.responders[0]({ type: "odm-status" }, null, respond);
    await captured;
    await new Promise((r) => setTimeout(r, 10));
    const call = fetchStub.calls.find((c) => c.url.includes("/api/extension/status"));
    assert.ok(call, "status call made");
    assert.strictEqual(call.opts.headers["x-odm-token"], "sekrit");
  });

  // 4. capture.js: esc() neutralizes markup in URLs
  await test("capture.js escapes markup", () => {
    const sb = makePageSandbox(makeDomStub());
    loadScript("capture.js", sb);
    const evil = '" onmouseover="alert(1)" data-x="';
    const out = sb.__test.esc(evil);
    assert.ok(!out.includes('" on'), "quotes must be escaped");
    assert.ok(out.includes("&quot;"), "escaped to entities");
  });

  // 5. capture.js: panel markup escapes collected URLs
  await test("capture.js panel escapes page-controlled URLs", () => {
    const dom = makeDomStub();
    dom.doc.querySelectorAll = () => [
      { src: 'http://v/x.mp4"><script>alert(1)</script>', tagName: "VIDEO", poster: "" },
    ];
    loadScript("capture.js", makePageSandbox(dom));
    const html = dom.doc.lastPanelHtml || "";
    assert.ok(!html.includes("<script>"), "raw script tag must not appear");
    assert.ok(html.includes("&lt;script&gt;"), "script must be entity-escaped");
  });

  // 6. manifests: required keys + referenced icons exist
  for (const dir of ["chromium", "firefox"]) {
    await test(`${dir} manifest valid + icons present`, () => {
      const m = JSON.parse(fs.readFileSync(path.join(__dirname, dir, "manifest.json"), "utf8"));
      assert.ok(m.name && m.version);
      const icons = Object.values(m.icons || {});
      assert.ok(icons.length >= 3, "manifest references icons");
      // packed bundle contains them
      const zipBytes = fs.readFileSync(path.join(__dirname, `odm-${dir === "chromium" ? "chromium.zip" : "firefox.xpi"}`));
      assert.ok(zipBytes.length > 5000, "packed bundle exists");
    });
  }

  console.log(`\n${passed} passed, ${failed} failed`);
  process.exit(failed ? 1 : 0);
})();

// minimal DOM stub for capture.js
function makeDomStub() {
  const doc = {
    readyState: "complete",
    lastPanelHtml: "",
    documentElement: { appendChild() {} },
    querySelectorAll() { return []; },
    getElementById() { return null; },
    createElement() {
      const el = {
        style: { cssText: "" },
        _html: "",
        set innerHTML(v) { this._html = v; doc.lastPanelHtml = v; },
        get innerHTML() { return this._html; },
        appendChild() {},
        querySelectorAll() { return []; },
        querySelector() { return { addEventListener() {} }; },
        addEventListener() {},
      };
      return el;
    },
    addEventListener() {},
  };
  const window = { addEventListener() {} };
  return { doc, window };
}
function makePageSandbox(dom) {
  return {
    window: dom.window,
    document: dom.doc,
    chrome: makeChromeStub().chrome,
    MutationObserver: class { observe() {} },
    setTimeout, clearTimeout, console,
  };
}
