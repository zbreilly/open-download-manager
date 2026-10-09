// Open Download Manager engine: C interface.
// Calls take and return UTF-8 JSON. Strings returned by odm_call must be
// released with odm_free. Events arrive on an engine thread.
#ifndef ODM_H
#define ODM_H
#include <stdint.h>

typedef void (*odm_event_cb)(const char *name, const char *json, void *ctx);

// Open the database in data_dir and start the engine and the browser
// extension bridge. Returns 0 on success; otherwise odm_last_error() explains.
int32_t odm_start(const char *data_dir, odm_event_cb cb, void *ctx);

// Call an engine method. Returns {"ok": <result>} or {"err": "<message>"}.
char *odm_call(const char *method, const char *args_json);

void odm_free(char *s);

// Last odm_start error (static string, do not free).
const char *odm_last_error(void);

// Save state and stop running downloads (call when quitting).
void odm_shutdown(void);
#endif
