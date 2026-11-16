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

// iOS touch controls (docs/runtime/ios.md, crates/input/src/touch.rs). The
// buttons held, as H2_TOUCH_* bits, named for the pad control they press.
enum {
    H2_TOUCH_A = 1u << 0, // Jump
    H2_TOUCH_B = 1u << 1, // Melee
    H2_TOUCH_X = 1u << 2, // Reload / action
    H2_TOUCH_Y = 1u << 3, // Switch weapon
    H2_TOUCH_BLACK = 1u << 4, // right shoulder: switch grenades
    H2_TOUCH_WHITE = 1u << 5, // left shoulder: flashlight
    H2_TOUCH_LEFT_TRIGGER = 1u << 6, // grenade
    H2_TOUCH_RIGHT_TRIGGER = 1u << 7, // fire
    H2_TOUCH_START = 1u << 8,
    H2_TOUCH_BACK = 1u << 9,
    H2_TOUCH_LEFT_THUMB = 1u << 10, // crouch (a toggle or held, per the setting)
    H2_TOUCH_RIGHT_THUMB = 1u << 11, // zoom
    H2_TOUCH_DPAD_UP = 1u << 12,
    H2_TOUCH_DPAD_DOWN = 1u << 13,
    H2_TOUCH_DPAD_LEFT = 1u << 14,
    H2_TOUCH_DPAD_RIGHT = 1u << 15,
};

typedef struct H2TouchFrame {
    uint32_t buttons;  // H2_TOUCH_* held
    bool move_active;  // a thumb is on the move stick
    float move_x;      // its offset from where it landed, points, +y down
    float move_y;
    float look_dx;     // look drag since the previous call, points, +y down
    float look_dy;
    float gyro_x;      // rotation rate, rad/s: +x turns right, +y looks up
    float gyro_y;
} H2TouchFrame;

// One report from the on-screen controls: on every touch change and once
// per display frame. Rust turns it into the pad (sticks, curve, crouch
// toggle) and merges it into player one. False with no game running.
bool h2host_touch_update(H2Host *host, const H2TouchFrame *frame);
// The on-screen controls showed or hid; hiding lets go of what they held.
void h2host_touch_shown(H2Host *host, bool shown);
// Gyro aiming's sensitivity from the settings, 0 when off.
float h2host_touch_gyro(H2Host *host);
// Whether crouch is held (a toggled crouch included), to light its button.
bool h2host_touch_crouched(H2Host *host);

// What the on-screen controls draw, from player one's state
// (crates/input/src/touch_layout.rs): a layout, the buttons shown and
// dimmed, and the move stick's radius.
enum {
    H2_TOUCH_LAYOUT_ON_FOOT = 0,
    H2_TOUCH_LAYOUT_DRIVER = 1, // Fire, Boost (left trigger), Exit (hold X), Zoom
    H2_TOUCH_LAYOUT_TURRET = 2, // passenger or gunner: Fire, Exit, Zoom
};
enum {
    H2_TOUCH_FLAG_ZOOM_NEAR_FIRE = 1u << 0, // a scoped weapon: Zoom large, by Fire
    H2_TOUCH_FLAG_DUAL = 1u << 1, // dual-wielding: left trigger is "FIRE L" on the left
    H2_TOUCH_FLAG_ON_TARGET = 1u << 2, // the reticle is red
};
typedef struct H2TouchLayout {
    uint32_t layout;     // H2_TOUCH_LAYOUT_*
    uint32_t visible;    // H2_TOUCH_* bits drawn
    uint32_t dimmed;     // of those, drawn faint
    uint32_t flags;      // H2_TOUCH_FLAG_*
    float stick_radius;  // points for a full move stick
    uint32_t generation; // changes whenever anything above does
} H2TouchLayout;
// Fills `out` with the layout to draw now. False with no game running.
bool h2host_touch_layout(H2Host *host, H2TouchLayout *out);

#endif
