// Checks the extension's sealing code against the app's (odm-core) with
// fixed messages kept in crates/odm-core/tests/data:
//   pairing-request.json   sealed here, must open in the app
//                          (cargo test -p odm-core pairing)
//   pairing-response.json  sealed by the app, must open here
// Run: node extension/test/pairing.test.mjs   (--write rewrites the request)
import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
createRequire(import.meta.url)(join(here, "../src/pairing.js"));
const { parseKey, importKey, sealRequest, openResponse } = globalThis.ODMPairing;
const data = join(here, "../../crates/odm-core/tests/data");

assert.equal(parseKey("zz"), null);
assert.equal(parseKey("01".repeat(31)), null);
assert.equal(parseKey(` ${"AB".repeat(16)}-${"cd".repeat(16)}\n`), "ab".repeat(16) + "cd".repeat(16));

const key = await importKey(parseKey("01".repeat(32)));
const nonce = Uint8Array.from({ length: 12 }, (_, i) => i);
const sealed = await sealRequest(key, "ping", { hello: "wörld" }, 1_800_000_000_000, nonce);
const requestFile = join(data, "pairing-request.json");
if (process.argv.includes("--write")) writeFileSync(requestFile, sealed.envelope + "\n");
assert.equal(readFileSync(requestFile, "utf8").trim(), sealed.envelope, "request differs from the stored one");
assert.ok(!sealed.envelope.includes("hello"), "the request is not readable");

const response = readFileSync(join(data, "pairing-response.json"), "utf8").trim();
assert.deepEqual(await openResponse(key, sealed.nonce, response), { ok: true, app: "Open Download Manager" });

// an answer for another request, or sealed with another key, is refused
const other = await sealRequest(key, "ping", null);
await assert.rejects(openResponse(key, other.nonce, response));
const stranger = await importKey(parseKey("02".repeat(32)));
await assert.rejects(openResponse(stranger, sealed.nonce, response));
// a request is not an answer
await assert.rejects(openResponse(key, sealed.nonce, sealed.envelope));

// large messages (thousands of links with cookies) survive the encoding
const big = { items: Array.from({ length: 5000 }, (_, i) => ({ url: `https://h.example/f${i}.zip`, cookies: "sid=" + "x".repeat(200) })) };
const bigSealed = await sealRequest(key, "add", big);
assert.ok(JSON.parse(bigSealed.envelope).c.length > 1_000_000);

console.log("pairing ok");
