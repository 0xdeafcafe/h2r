// The tvOS and iOS apps' Rust half (crates/apple-host). All calls on the
// main thread.
#ifndef H2HOST_H
#define H2HOST_H

#include <stdbool.h>
#include <stdint.h>

typedef struct H2Host H2Host;

// Boots the game into `layer` (a CAMetalLayer*, width x height drawable
// pixels). `bundle` is the app bundle path, `caches` the Caches directory,
// `documents` the Documents directory where saves persist (iOS), or NULL
// to keep them in Caches (tvOS). Never NULL: check h2host_status.
H2Host *h2host_start(void *layer, uint32_t width, uint32_t height, const char *bundle,
                     const char *caches, const char *documents);
// Draws the newest game frame; call from a CADisplayLink.
void h2host_redraw(H2Host *host);
void h2host_resize(H2Host *host, uint32_t width, uint32_t height);
void h2host_set_paused(H2Host *host, bool paused);
// 0 running, 1 the game returned, 2 failed or never started.
int32_t h2host_status(H2Host *host);
// Why it is not running (UTF-8), or "". Valid until the next call.
const char *h2host_message(H2Host *host);

// The game's virtual keyboard (docs/input/host-keyboard.md). Once per frame:
// 1 a text field opened (show the system keyboard), -1 it closed (hide it),
// 0 nothing new.
int32_t h2host_text_edge(H2Host *host);
// What the system keyboard typed into the open field; false if none is open.
// insert takes committed UTF-8 text; a "\n" in it is Done.
bool h2host_text_insert(H2Host *host, const char *text);
bool h2host_text_delete_backward(H2Host *host);
bool h2host_text_done(H2Host *host);
// Restores the field's text and backs out, as the game's Back does. Hiding
// the system keyboard is not a cancel.
bool h2host_text_cancel(H2Host *host);

// iOS on-screen controls: whether a connected controller counts as a pad
// that can play (not the overlay's own, and with an extended profile), and
// whether the overlay should be connected.
bool h2host_pad_counts(bool is_overlay, bool extended);
bool h2host_overlay_wanted(uint32_t physical_pads, bool keyboard_shown);
// The host's own Start (0) and Back (1) buttons, held while `down`; the
// on-screen controls have neither.
bool h2host_button(H2Host *host, int32_t button, bool down);

#endif
