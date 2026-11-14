// Open Download Manager — what a page's network responses are, for the
// media list: direct audio/video files, HLS/DASH stream manifests, or the
// segments of a stream (never listed on their own).
// Loaded by the background (service worker: importScripts; Firefox: the
// manifest's script list) and by the tests.
"use strict";
(function () {
  const MEDIA_MIME = /^(video|audio)\//i;
  const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|flv|wmv|mp3|m4a|aac|flac|wav|ogg|oga|opus)(\?|#|$)/i;
  const HLS_MIME = /^(application|audio)\/(vnd\.apple\.mpegurl|x-mpegurl|mpegurl)$/i;
  const DASH_MIME = /^application\/dash\+xml$/i;
  // pieces of a stream: MPEG-TS and (CMAF) fragments
  const SEGMENT_EXT = /\.(ts|m4s|cmfv|cmfa|m2ts|mts)(\?|#|$)/i;
  const SEGMENT_MIME = /^(video\/(mp2t|iso\.segment)|audio\/iso\.segment)$/i;

  function pathOf(url) {
    try { return new URL(url).pathname; } catch { return String(url || "").split(/[?#]/)[0]; }
  }

  /**
   * What a response is: "hls" or "dash" (a stream manifest), "media" (a
   * direct audio/video file), or "" (nothing to list: a segment, or not
   * media at all). `mime` is the Content-Type without parameters.
   */
  function classify(url, mime) {
    const m = String(mime || "").split(";")[0].trim();
    const path = pathOf(url).toLowerCase();
    if (HLS_MIME.test(m) || path.endsWith(".m3u8")) return "hls";
    if (DASH_MIME.test(m) || path.endsWith(".mpd")) return "dash";
    if (SEGMENT_MIME.test(m) || SEGMENT_EXT.test(path)) return "";
    if (MEDIA_MIME.test(m) || MEDIA_EXT.test(path)) return "media";
    return "";
  }

  /** A file name from a page title ("" when there is none). */
  function nameFromTitle(title) {
    const t = String(title || "")
      .replace(/[\\/:*?"<>|\u0000-\u001f]+/g, " ")
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, 120)
      .replace(/[. ]+$/, "");
    return t;
  }

  /** The name to show and suggest for a stream manifest. */
  function streamName(url, title) {
    const t = nameFromTitle(title);
    if (t) return t;
    const last = decodeURIComponent(pathOf(url).split("/").pop() || "").replace(/\.(m3u8|mpd)$/i, "");
    return last || "stream";
  }

  const api = { classify, nameFromTitle, streamName };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  globalThis.ODMMedia = api;
})();
