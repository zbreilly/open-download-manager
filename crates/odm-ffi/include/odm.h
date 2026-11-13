// Open Download Manager engine: C interface.
//
// Calls take and return UTF-8 JSON. Strings returned by odm_call must be
// released with odm_free.
//
// Threading and safety contract:
// * Every function may be called from any thread. No Rust panic ever
//   crosses this interface: a panic inside odm_call is returned as
//   {"err": "internal error: ..."}; inside odm_start as -5.
// * Calls that wait for the engine (odm_start, odm_shutdown and the
//   methods probe_url, refresh_link and grab) must not be made from a thread
//   that runs the engine's async code. Such calls are refused with an error
//   instead of blocking (odm_start returns -6; odm_call returns
//   {"err": ...}; odm_shutdown saves state without waiting).
// * Events are delivered one at a time, in order, on a single dedicated
//   thread named "odm-events" (never on the caller's thread). The callback
//   may call odm_call; a slow callback delays later events, never the engine.
//   `name` and `json` are only valid during the callback.
#ifndef ODM_H
#define ODM_H
#include <stdint.h>

// May be NULL (no events).
typedef void (*odm_event_cb)(const char *name, const char *json, void *ctx);

// Open the database in data_dir and start the engine and the browser
// extension bridge. Safe to call concurrently or repeatedly: only the first
// successful call starts the engine (later calls return 0 and their cb/ctx
// are ignored). ctx is passed back to cb unchanged.
// Returns 0 on success; otherwise odm_last_error() explains:
//   -1 bad data_dir, -2 cannot create data_dir, -3 runtime, -4 database,
//   -5 internal error (panic), -6 called from an engine thread.
// If the bridge port is taken the engine still starts: a "bridge_error"
// event {port, error} is emitted and extension_status reports bridgeError.
int32_t odm_start(const char *data_dir, odm_event_cb cb, void *ctx);

// Call an engine method. Returns {"ok": <result>} or {"err": "<message>"}.
// Never returns NULL. Free the result with odm_free.
// save_settings with a new extensionPort first listens on the new port;
// if that fails nothing is saved, the old listener keeps running, a
// "bridge_error" event is emitted and {"err": ...} is returned.
// bridge_key returns the key the browser extension must be paired with
// (show it in the settings so the user can paste it into the extension);
// regenerate_bridge_key replaces it and returns the new one. The bridge
// only acts on requests sealed with this key.
char *odm_call(const char *method, const char *args_json);

// Free a string returned by odm_call (NULL is ignored).
void odm_free(char *s);

// Last error of odm_start / odm_shutdown on the calling thread ("" if none).
// Owned by the library (do not free). Valid until the next call into this
// library on the same thread; copy it if you need it longer.
const char *odm_last_error(void);

// Stop the bridge and all downloads, wait (up to 5 seconds) for them to
// flush, and save state. Call when quitting, from a non-engine thread.
// Runs once; later calls return immediately.
void odm_shutdown(void);
#endif
