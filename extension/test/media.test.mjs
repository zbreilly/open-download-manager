// Checks how the extension sorts a page's network responses for its media
// list: direct files, HLS/DASH manifests, and stream segments (never listed).
// Run: node extension/test/media.test.mjs
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const { classify, streamName, nameFromTitle } = createRequire(import.meta.url)(join(here, "../src/media.js"));

// manifests, by content type or by name
assert.equal(classify("https://cdn.example/v/master.m3u8?token=abc", ""), "hls");
assert.equal(classify("https://cdn.example/v/play", "application/vnd.apple.mpegurl"), "hls");
assert.equal(classify("https://cdn.example/v/play", "application/x-mpegURL"), "hls");
assert.equal(classify("https://cdn.example/v/play", "audio/mpegurl"), "hls");
assert.equal(classify("https://cdn.example/v/manifest.mpd", "text/xml"), "dash");
assert.equal(classify("https://cdn.example/v/play", "application/dash+xml"), "dash");
// segments of a stream are not media to list
assert.equal(classify("https://cdn.example/v/seg-00012.ts", "video/mp2t"), "");
assert.equal(classify("https://cdn.example/v/chunk-1.m4s", "video/mp4"), "");
assert.equal(classify("https://cdn.example/v/frag", "video/iso.segment"), "");
// direct files
assert.equal(classify("https://cdn.example/clip.mp4", "video/mp4"), "media");
assert.equal(classify("https://cdn.example/song", "audio/mpeg"), "media");
assert.equal(classify("https://cdn.example/song.opus?x=1", ""), "media");
// anything else
assert.equal(classify("https://cdn.example/page.html", "text/html"), "");
assert.equal(classify("https://cdn.example/app.js", "application/javascript"), "");

// names
assert.equal(nameFromTitle('A "video": part 1/2 '), "A video part 1 2");
assert.equal(streamName("https://cdn.example/v/master.m3u8", "My Show – Episode 5 | Site"), "My Show – Episode 5 Site");
assert.equal(streamName("https://cdn.example/v/lecture-01.mpd", ""), "lecture-01");
assert.equal(streamName("https://cdn.example/", ""), "stream");
console.log("media: ok");
