// Send one request to a running app's browser bridge, sealed with the
// pairing key exactly as the extension seals it (it uses the extension's own
// code), and print the app's answer.
//
// usage: ODM_BRIDGE_KEY=<64 hex digits> node bridge-rpc.mjs PORT OP [JSON_BODY]
// exit:  0 answer with ok != false · 1 the app said no · 2 not accepted (HTTP status printed)
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
createRequire(import.meta.url)(join(here, "../../extension/src/pairing.js"));
const { parseKey, importKey, sealRequest, openResponse } = globalThis.ODMPairing;

const [port, op, body] = process.argv.slice(2);
const hex = parseKey(process.env.ODM_BRIDGE_KEY);
if (!port || !op || !hex) {
  console.error("usage: ODM_BRIDGE_KEY=<64 hex digits> node bridge-rpc.mjs PORT OP [JSON_BODY]");
  process.exit(64);
}
const key = await importKey(hex);
const sealed = await sealRequest(key, op, body ? JSON.parse(body) : null);
const r = await fetch(`http://127.0.0.1:${port}/api/rpc`, {
  method: "POST",
  headers: { "content-type": "application/json", "x-odm-browser": "CI" },
  body: sealed.envelope,
});
if (r.status !== 200) {
  console.log(`HTTP ${r.status}`);
  process.exit(2);
}
const answer = await openResponse(key, sealed.nonce, await r.text());
console.log(JSON.stringify(answer));
process.exit(answer && answer.ok === false ? 1 : 0);
