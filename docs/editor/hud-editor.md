# The HUD editor

Status: Built. Code: `crates/hud-edit/src`, the HUD tool in `crates/studio/src`. Tests: in-source unit tests.
Switch: none.

The editor's **HUD** tool shows a map's HUDs (`nhdt` tags) as the game
places them on a 640 by 480 screen, at a state you pick (shield, health,
grenades, ammunition, zoom, split screen), and edits their widgets: drag
one to move it, change its anchor, offset, registration, sequence,
bitmap, shader, inputs or state flags, and save the map as a new `.map`
that differs from the source only in the edited bytes. It flags art that
partly covers other art, art off the screen, and text that spills out of
the art it sits on, which is how the layout bugs of converted Halo 1 HUDs
show (a shield bar over the health bar, a grenade count outside its box).

To use it, read the guide: [editor-guide.md](editor-guide.md#8-hud).
This page is the developer's view: where the logic lives, what each
placement rule rests on, and what is not modelled.

Labels, as elsewhere: **E** engine code in the retail `default.xbe`
(read with `h2 disasm`), **V** seen on the owner's disc or in a frame of
the game, **D** assumed here.

## Code

- `crates/hud-edit` (no UI):
  - `layout`: the `nhdt` root (`+0x08` bitmap widgets, `0x64` each;
    `+0x10` text widgets, `0x54` each; E `0x13a532`) and every widget
    field with the address that reads it; the field tables
    (`BITMAP_FIELDS`, `TEXT_FIELDS`) that edits by key use (`HudSession::set`)
    and the definition `tags/src/defs/nhdt.txt` agrees with.
  - `place`: the placement maths and `RULES`, each with its proof.
  - `state`: `HudState` (what the HUD is drawn at), the inputs it gives
    (`HudState::input`), and the engine's widget gate (`shown`).
  - `model`: `list` (every `nhdt`, with the bipeds, vehicles, weapons and
    globals that name it) and `read` (a HUD's widgets as their bytes at
    their metadata addresses).
  - `render`: the preview canvas from the map's bitmaps (`tags` decode,
    `assets::decode_image`) and the disc's fonts (`font`), and where each
    widget landed (`Placed`: point, rectangle, shown, covered pixels).
  - `checks`: the findings.
  - `session`: `HudSession`: the open map, edits by field key, by offset
    (`move_to`) or by address and bytes (`write`, for the field panel),
    undo and redo (a drag is one step), and `save_as` through
    `map::writer::MapEditor`.
  - `fixture` (feature `fixture`): an invented map for tests, built with
    `map::build` (its bitmap through `h1bitmaps`).
- `crates/studio`: `app/hud.rs` (`HudTool`: the session, the state, the
  selection, the picture and findings; the `Hud*` actions) and
  `ui/hud.rs` (the windows: maps, HUDs and widgets in the outliner; the
  picture and the state in the centre; the generic field panel, checks,
  HUDs drawn with it, placement rules and save in the inspector).

## Placement

A widget's point is its anchor's point plus the offset of the screen
mode; the art is placed so its registration point lands on that point.

| Rule | Proof |
| --- | --- |
| The anchor point is a corner of the view's HUD frame, the render camera's `+0x38` rectangle (`0x4b9dd8`, i16 top, left, bottom, right): 0 top right, 1 top left, 2 bottom left, 3 bottom right, 4 the frame's centre (integer midpoints) | E `0x1396c7` (`0x139908`, `0x1398f4`, `0x1398eb`, `0x1398e2`, `0x13978a`) |
| Anchor 4: with a live player view the centre's y is the projected aim point (`0x16f3a0`, `0x142570`); the preview uses the frame's centre | E `0x139799`..`0x1398bc` |
| Anchors above 4 go another way (a projected point, not read); the preview draws them at the centre and says so | E `0x1396f4` |
| Fullscreen at 640 x 480 the frame's bottom-left corner is (48, 444) | V a frame of a10 (motion tracker rim x 47..129, y 360..442), the health strip's tests in `h1objects::health_hud` |
| The frame is the screen inset 7.5% on each side, (48, 36) to (592, 444) | D (symmetric with the corner seen) |
| Split-screen frames: the top half or top-left quarter, inset 7.5% | D |
| Bitmap: point = anchor + offset (`+0x34`, `+0x38`, `+0x3c` by mode); the sprite spans `point - reg * size` to `point + (1 - reg) * size`, `reg` the registration (`+0x40`, `+0x48`, `+0x50`, fractions) | E `0x22aff1`, `0x22b128`..`0x22b179`, `0x22b1eb`..`0x22b234` |
| Sprite: the mode's sequence (`+0x30`..`+0x32`); with sprites, the first sprite's bounds in its image; without, the sequence's first image whole; a bitmap with no sequences, image 0 | E `0x22ae21`, `0x22ae94`..`0x22af3c` |
| Some sequences draw their second image (`0x22acb4`); the preview draws the first | D (condition not read) |
| Sprites are 1:1 in HUD pixels at 640 x 480 fullscreen | V a10 (the tracker's 83 x 83) |
| Text: point = anchor + offset (`+0x40`, `+0x44`, `+0x48`); justification `+0x2c` 0 starts at it, 1 centres on it, 2 ends at it; the text's top is the point | E `0x22aa16`, `0x22ab17`..`0x22ac64` |
| Text font: the mode's byte `+0x30`..`+0x32`, 0 engine font 6, 1 engine font 8 (a `font_table.txt` line), anything else no text | E `0x22a85b`, `0x22a98e`..`0x22a9b6` |
| Text: no shader or no string, no draw | E `0x22aa26`, `0x22aa2f` |
| Widget effects (`+0x58`, text `+0x4c`) animate widgets; not modelled | E `0x22bcaf` |
| Bitmap flags `+0x1e` bits 0 and 1 mirror the texture | E `0x22b29c` (docs/halo1/health.md) |
| Meters (`ammo_meter`, template `widget_legacy_meter`) fill where the texture's blue is below `hud_input1`; the shield meter (special type 5) by the shield through its shader's span 0.2156..0.992; other shaders' colours are not modelled | V docs/halo1/health.md |

The **state gate** (E `0x13ac87`, from `0x13a532`): the 20 state bytes
at `+0x08` hold four show words (A, B, C, D) and four hide words; the
widget is drawn when one show word meets the matching state word and no
hide word does. Bytes `+0x18..+0x1a` are thresholds the engine turns into
word C's bits `0x08`, `0x10`, `0x20` from the weapon's state
(`0x13ac8f`..). Which game state sets which bit is mostly not read: the
preview sets word A `0x04` with the frag grenade selected and `0x08` with
the plasma (V: the retail grenade widgets and counts), and `0x40` unzoomed
(V: the retail scope masks hide on it); the rest are typed in. The gate is
off by default (every widget drawn).

**Inputs** (E `0x13ad48`) the state sets: 16 shield, 17 body, 20 frag
and 21 plasma grenade counts, `0x31` heat, `0x36` rounds loaded over the
magazine, `0x38` battery. Any other input is 1 unless typed in.

## Checks

On the widgets drawn (`checks::check`):

- **off the screen** (warning) and **outside the HUD frame** (note:
  retail art sits a pixel outside it);
- **art overlaps** (warning): two bitmap widgets share at least four
  opaque pixels (alpha over a quarter) and neither's rectangle holds the
  other's. Nested rectangles are layers by design (a panel over its
  meter, a flash over its bar) and are not flagged;
- **text spills** (warning): a text's rectangle crosses a bitmap
  widget's but is inside none of them;
- **texts overlap** (warning).

## The save

`HudSession::save_as` opens the source again and queues one
`MapEditor::patch_metadata` per run of changed bytes in each edited
widget (at the widget's metadata address plus the run's offset), then
writes to a sibling temporary file renamed into place, never over the
source. The output is the source byte for byte but for those runs and
the header checksum (`+0x2d0`). A tag reference's edit changes its datum
only. Nothing grows, so nothing moves.

## The field panel

The inspector shows the selected widget's element in the studio's generic
field panel (`studio::fields::panel`, the Tags tool's), named by the `nhdt`
definition `crates/tags/src/defs/nhdt.txt`: anchor (an enum), flags,
inputs, the show and hide words of the gate (word A's known bits named),
bitmap and shader references with the tag picker, sequences, offsets and
registrations per screen mode, the special type, and the effects blocks
(shown; their elements are the Tags tool's).

The HUD tool keeps one history and one save: its own `TagDoc` is loaded
with the session and only shows bytes. The panel's writes come back as
`Action::HudWrite` and go to `HudSession::write` (address and bytes, which
must fall inside one widget record), and after every change the session's
widget bytes are mirrored into the document (`TagDoc::mirror`, no undo
step). Block changes are refused with a note.

`+0x18..+0x1a` (both widget kinds) are the gate's threshold bytes
(`threshold_battery`, `threshold_loaded`, `threshold_reserve`; E
`0x13ac8f`..`0x13acbd`) and the text widgets' `+0x2c` is the
justification (E `0x22ac0b`). Inference once typed the first as a
raw-resource pointer and the second as its size, so a fresh or
from-scratch map build rewrote the thresholds as a file offset (V: a
converted Blood Gulch's "Reload" widget, `00 0b 01 00` written, `00 da 22
01` built). The schema now leaves that word out
(`tags::schema::infer::ENGINE_DATA`, docs/formats/tag-schema.md step 9)
and the definition types the bytes. The HUD tool still keeps a row from
`hud_edit::layout`'s tables for the justification. A test
(`the_nhdt_definition_agrees_with_the_layout_tables`) keeps the
definition and the tables together.

## Not modelled

- Widget effects (animation, flashing, scaling).
- Shader colours: art is drawn in its own texture colours; text is drawn
  in one light blue.
- Text strings: a text that is not a number shows its string ID's name
  in angle brackets (the localized text is not looked up).
- The motion tracker's blips and the engine's own centre table (see
  docs/halo1/hud.md).
- Split-screen frames (D above).
- The second-image choice of `0x22acb4`.

## Tests

- `cargo test -p hud-edit`: the field tables fit their widgets; the
  anchor corners and the retail `ui\hud\masterchief` shield meter and
  tracker rectangles as documented; registration and justification; the
  gate on the retail grenade and scope flags; inputs; over the invented
  map: listing with owners, reading, placement and pixels, meter fill,
  the checks (the stray overlap, a spilling number, off the screen), the
  gate, undo and redo with a drag as one step, and a save that differs
  from the source only in the edited bytes and reads back.
- `H2_DISC=... cargo test -p hud-edit -- --ignored`: Lockout's
  `ui\hud\masterchief` shield meter at x 47..175, y 346..378 and tracker
  at x 47..130, y 360..443, and every drawn bitmap widget of every HUD on
  the screen.
- `cargo test -p studio hud`: the tool's actions (open, hit testing,
  a drag as one undo step, the dirty marker, the unsaved-edits prompt and
  Save as) and `headless::tests::the_hud_tool_picks_and_drags_a_widget`,
  which picks the map and a widget and drags it on the picture through
  the test-engine hooks.
