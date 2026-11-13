// Open Download Manager — sealed messages between the extension and the app.
//
// The app listens on a loopback port. A port says nothing about who is
// behind it, so the extension and the app share a key (shown in the app's
// settings, pasted into the extension once) and seal every message with it
// (AES-256-GCM). Whoever merely owns the port can neither read what the
// extension sends (links and their cookies) nor fake the app's answers.
// The app's side is crates/odm-core/src/pairing.rs.
"use strict";
(() => {
  const AAD_REQUEST = "odm-bridge-v1 request";
  const AAD_RESPONSE = "odm-bridge-v1 response ";
  const enc = new TextEncoder();
  const dec = new TextDecoder();

  function b64(bytes) {
    let s = "";
    for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s);
  }

  function unb64(text) {
    const s = atob(text);
    const out = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
    return out;
  }

  /** The key as 64 lowercase hex digits, or null if `text` isn't a key (spaces and dashes are ignored). */
  function parseKey(text) {
    const clean = String(text ?? "").replace(/[\s-]/g, "");
    return /^[0-9a-fA-F]{64}$/.test(clean) ? clean.toLowerCase() : null;
  }

  /** CryptoKey for a key accepted by parseKey. */
  function importKey(hex) {
    const raw = new Uint8Array(32);
    for (let i = 0; i < 32; i++) raw[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
    return crypto.subtle.importKey("raw", raw, "AES-GCM", false, ["encrypt", "decrypt"]);
  }

  /**
   * Seal a request. Returns the envelope to send and the nonce needed to
   * open the answer. `now` and `nonce` are fixed only by tests.
   */
  async function sealRequest(key, op, body, now = Date.now(), nonce = crypto.getRandomValues(new Uint8Array(12))) {
    const plain = enc.encode(JSON.stringify({ ts: now, op, body }));
    const sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce, additionalData: enc.encode(AAD_REQUEST) }, key, plain);
    const n = b64(nonce);
    return { envelope: JSON.stringify({ v: 1, n, c: b64(new Uint8Array(sealed)) }), nonce: n };
  }

  /**
   * Open the answer to the request sealed with `nonce`. Throws unless it
   * was sealed with the same key for exactly that request.
   */
  async function openResponse(key, nonce, envelope) {
    const env = JSON.parse(envelope);
    if (!env || env.v !== 1 || typeof env.n !== "string" || typeof env.c !== "string") throw new Error("not a sealed answer");
    const plain = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: unb64(env.n), additionalData: enc.encode(AAD_RESPONSE + nonce) },
      key,
      unb64(env.c)
    );
    return JSON.parse(dec.decode(plain));
  }

  globalThis.ODMPairing = { parseKey, importKey, sealRequest, openResponse };
})();
