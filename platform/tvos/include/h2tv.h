// The tvOS app's Rust half (crates/tvos). All calls on the main thread.
#ifndef H2TV_H
#define H2TV_H

#include <stdbool.h>
#include <stdint.h>

typedef struct H2Tv H2Tv;

// Boots the game into `layer` (a CAMetalLayer*, width x height drawable
// pixels). `bundle` is the app bundle path, `caches` the Caches directory.
// Never NULL: check h2tv_status.
H2Tv *h2tv_start(void *layer, uint32_t width, uint32_t height, const char *bundle, const char *caches);
// Draws the newest game frame; call from a CADisplayLink.
void h2tv_redraw(H2Tv *tv);
void h2tv_resize(H2Tv *tv, uint32_t width, uint32_t height);
void h2tv_set_paused(H2Tv *tv, bool paused);
// 0 running, 1 the game returned, 2 failed or never started.
int32_t h2tv_status(H2Tv *tv);
// Why it is not running (UTF-8), or "". Valid until the next call.
const char *h2tv_message(H2Tv *tv);

#endif
