# Button icons for the device in use

Status: Built. Unit tested on synthetic fonts; the game's own text path is under "Not verified".
Code: `crates/extensions/game/src/icons.rs`, `crates/formats/font/src/icons/`, `crates/platform/input/src/active.rs`. Tests: listed under "Tests".
Switch: `H2_ICONS` (`--icons`), see [switches.md](../runtime/switches.md).

Halo 2 draws button prompts ("Press X to pick up", the menu legends, the
on-screen keyboard's shortcuts) as glyphs in its fonts. They show the original
Xbox pad. This page says where those glyphs are, and how the runner shows
Xbox 360, PlayStation or keyboard and mouse pictures in their place, following
whichever device the player last touched.

```sh
h2 play disc.iso --icons auto|xbox|xbox360|playstation|keyboard|remote|steam
H2_ICONS=playstation h2 run disc.iso ...
```

`auto` is the default in `h2 play`. `h2 run` changes nothing unless `H2_ICONS`
is set, so headless runs and benchmarks see the retail fonts.

No glyph data is in the repository. The pictures are computed at run time from
shapes in `crates/formats/font/src/icons/`, and added to the user's own font files as
the game reads them. Nothing on disc is modified and no vendor logo or artwork
is used: a cross, circle, square and triangle are drawn as geometry.

## The retail glyphs

Read from the disc with a scratch tool over `crates/formats/font` (the format is in
[fonts.md](../formats/fonts.md)); `h2 font <iso> conduit-16 <text> --glyph-colors` draws
any of them. The text scanner treats U+E000 to U+E3FF as its icon range. Every
font file on the disc, the Asian ones included, maps the same pictures at the
same code points:

| Code point | Picture |
|---|---|
| U+E100 to U+E103 | A, B, X, Y |
| U+E104, U+E105 | Black, White |
| U+E106, U+E107 | left trigger, right trigger |
| U+E108 to U+E10B | d-pad up, down, left, right |
| U+E10C, U+E10D | Start, Back |
| U+E10E, U+E10F | left stick, right stick |
| U+E110, U+E111 | left stick click, right stick click |
| U+E12C to U+E130 | five more stick pictures, not identified (untouched) |

The rest of the range holds other things: emoticons and small symbols
(U+E000 to U+E008), team colour swatches (U+E010 to U+E027), medals in colour
and in outline (U+E033 to U+E099) and weapon outlines (U+E112 to U+E12B).
Unused code points up to U+E3FF all point at one filler record (a box), or at
nothing in `arial-*` and `fixedsys-9`. Nothing on the disc uses U+E200 or above.

How they are stored, in the HUD and menu fonts (`conduit-*`, `handel_gothic-*`,
`MSLCD-14`):

| Glyphs | Size | Colour | Stream |
|---|---|---|---|
| A, B, X, Y | 16 x 18 | a shaded ball in the button's colour, light letter; 36 to 70 distinct 12-bit colours, all 16 alpha levels on the edge | 560 to 640 bytes, mostly literal words |
| Black, White, Start, Back | 16 x 18 | grey shaded balls (12 to 17 colours); Start and Back carry a light triangle | 450 to 510 bytes |
| Triggers | 45 x 19 | **both** triggers side by side, the one meant lit and the other faint; white only | about 325 bytes |
| Sticks | 38 x 19 | both sticks, one lit; the click pictures add a solid outline to the lit one; white only | 240 to 265 bytes |
| D-pad | 16 x 18 and 9 x 18 | a plain white triangle | 30 to 36 bytes |

- Every icon has `x_offset` 0, `advance` equal to its width and `y_offset`
  equal to the font's ascent, so its top sits on the ascent line.
- The white-only pictures use just the eight alpha levels a run byte can carry
  (0, 3, 4, 7, 8, 11, 12, 15 of 15), so they need no literal words. The faint
  member of a pair is alpha 3 with a 4 edge; the lit one is 8 with 11 to 15 on
  its edge. Being white, they take the colour of the text they sit in. The
  coloured ones are drawn in their own colours.
- All of them are compressed with the scheme in [fonts.md](../formats/fonts.md); none is
  stored as plain rows.
- Sizes vary a little by file: `handel_gothic-11` has 16 x 16 face buttons, and
  `handel_gothic-14` and `MSLCD-14` have shorter d-pad arrows (16 x 11) and
  18-pixel pairs. `arial-*` and `fixedsys-9` (not used by the HUD) have older
  single-control pictures for triggers and sticks, in colour, and lack
  U+E12C to U+E130.

## What each set shows

A set replaces only what differs from the Xbox pad. Where the retail picture is
already right for the device it stays, which is the best match for style there
is.

| Xbox control | Xbox 360 | PlayStation | Keyboard and mouse |
|---|---|---|---|
| A, B, X, Y | retail (same colours and letters) | cross, circle, square, triangle | the bound key or mouse button |
| Black | right bumper | R1 | bound key |
| White | left bumper | L1 | bound key |
| Triggers | retail pair | labelled L2/R2 pair | bound key or mouse button |
| Start, Back | retail (triangles) | Options, Share | bound key |
| Sticks | retail pair | retail pair | "WASD"-style cap, or the mouse for look |
| Stick clicks | retail | retail (L3, R3) | bound key or mouse button |
| D-pad | retail | retail | bound key |

Style rules, taken from the retail glyphs:

- **Face buttons** (PlayStation): a ball shaded like the retail A, B, X and Y,
  with a thin bevel and a small upper-left highlight, in the button's colour (blue,
  red, pink, green), 16 pixels wide, with the symbol in a light tint as the
  retail letters are. Options and Share are dark balls with a white mark, as
  the retail Start and Back are.
- **Bumpers** (360 and PlayStation): both bumpers side by side with the one
  meant lit, as the retail glyphs show triggers and sticks, in white at
  the same alpha levels, so they take the text colour. The pair is 45 pixels
  wide and carries LB/RB or L1/R1 labels; PlayStation triggers carry L2/R2.
  The inactive side stays faint. Labels keep their orientation when the
  active side changes.
- **Keys**: a light cap shaded like the retail White button, with a dark rim,
  a lip along the bottom and the key's name in a 5 x 7 letter set drawn for
  this project. **Mouse buttons**: a mouse from above in the same shading with
  the button lit in amber; mouse look shows the mouse between two arrows.
- Every icon takes the height and vertical offset of the retail glyph it
  stands for in that font, and its advance is its width, so it sits on the
  line where the retail one does. Widths differ (a "SPACE" cap is 37 pixels;
  the retail trigger pair is 45), and the game lays text out from the advance.

There is no Guide, PS or touchpad picture: the game has no code point that
means them.

**Apple TV remote** (`remote`, tvOS; [tvos.md](../runtime/tvos.md)). It replaces only
what the remote has, as its menu mapping (`input::backend::game_controller::remote`)
uses it, and every other button keeps the retail Xbox glyph:

| Xbox control | Remote picture |
|---|---|
| A | the clickpad: a faint ring with a filled centre |
| D-pad up, down, left, right | the clickpad ring with that quarter lit and a band inside it |
| B | a "<" back chevron in a faint ring |
| Start | play/pause: a triangle and two bars |

All four are white at the retail alpha levels, like the d-pad glyphs, so they
take the text colour. They are plain geometry: no Apple artwork or logo.
`auto` picks the set when the remote was the device last used (its pad name,
`Apple TV Remote`, is `input::active::DeviceKind::Remote`).

**Steam Controller** (`steam`). Its A, B, X and Y sit where the Xbox pad's do
and its triggers are analog, so those keep the retail glyphs. It replaces the
rest:

| Xbox control | Steam picture |
|---|---|
| Black, White | the LB/RB bumper pair, as the Xbox 360 set draws it |
| Start | a ">" chevron in a faint ring |
| Back | a "<" chevron in a faint ring |
| D-pad up, down, left, right | the clickpad ring with that quarter lit (a click on the left trackpad) |
| Right stick, right stick click | the clickpad with a filled centre (the right trackpad) |

All are white at the retail alpha levels, so they take the text colour, and
plain geometry: no Valve artwork or logo. `auto` picks the set when a Steam
Controller was the device last used (its pad name, `Steam Controller`, is
`input::active::DeviceKind::Steam`).

### Which button a prompt shows

The game chooses the button code point before the font lookup. The runner
maps that virtual Xbox control to the physical input driving it. It does not
change the game's action table or add controller layouts. A layout change
that updates the game's prompt code point therefore gets the corresponding
host icon; hardcoded guest prompts would still need fixing where emitted.
The mapping is:

- pads: `crates/platform/input` maps the right bumper to Black and the left to White,
  so Black draws the right bumper (RB, R1) and White the left;
- keyboard: the first input bound to the control in the user's bindings
  ([input.md](input.md), `--bindings`). A stick shows its four keys as one cap
  when each has a one-character name ("WASD", "IJKL"), otherwise its "up" key.
  With no keys on the right stick and mouse look on, it shows the mouse. A
  control with nothing bound shows a dash cap, rather than suggesting a
  physical button that cannot be pressed.

## How it works

Two parts, both in `crates/extensions/game/src/icons.rs`.

**1. The fonts get every set.** `vfs::RewriteBackend` wraps the disc. When
the game opens a file under `maps\fonts` (not the `.txt` tables), the whole
file is read once, `font::icons::replacements` draws the sets for that
font, and `font::patch::apply` appends them. Each set has code points of its
own, in the unused part of the icon range:

| Set | Code points |
|---|---|
| retail | U+E100 to U+E111 |
| Xbox 360 | U+E200 to U+E211 |
| PlayStation | U+E220 to U+E231 |
| keyboard and mouse | U+E240 to U+E251 |
| Apple TV remote | U+E260 to U+E271 |
| Steam Controller | U+E280 to U+E291 |

`apply` gives each new code point a new record at the end of the record
table, moves the pixel data up by the size of those records, fixes every
offset and the header's totals, and appends the new streams. The maximum
stream length and pixel total are recomputed from the final records, including
when a replacement shrinks. Splitting a shared record retains its placement.
The result passes
`Font::validate`, and every retail glyph decodes as before. A font that
already had a glyph of its own in those ranges is left alone (none on
the disc does). A shared record is accepted as filler only when it also
matches the missing-character record at U+FFFF and the uniform U+E300..E3FF
tail. Sharing an ordinary symbol between two code points is not enough.
The resulting font is fully validated before it is served.

If any opened font fails to patch, substitutions are disabled. If a font
lacks a source button, that substitution is disabled across all fonts for
the rest of the run. This conservative fallback keeps prompts on their
original code points instead of showing a missing glyph or a mod's unrelated
artwork. The original code points always remain available.

`font::encode` is the compressor: runs for transparent and
opaque pixels, pair and single bytes for the eight run alphas, and a literal
word wherever the colour changes or the alpha is not a run level.

**2. The lookup picks a set.** The game keeps decoded glyphs in a cache keyed
by font and code unit (0x200 entries of 0x38 bytes behind `0x54d574`, a hash
behind `0x54d570`; lookup `0x140b20`, insert with least-recently-used eviction
`0x140b80`). Everything that measures or draws a character reaches it through
`0x1407d0` (`eax` font id, first stack argument the code unit), and one more
path, `0x1f490`, takes the code unit as its third stack argument. The runner
overrides both: if the unit is U+E100 to U+E111 and the showing set has a
picture for it, the argument is rewritten to that set's code point, and the
original function runs.

Because the cache is keyed by code unit, **changing set needs no flush**. The
next string drawn asks for other code units; the old glyphs age out. So the
icons can follow the device live.

**Following the device.** The player-one pad that combines the keyboard with
the first physical controller (`KeyboardFallback`) notes fresh input in `input::active::ActiveDevice`. Bound key presses,
mouse buttons and mouse-look motion count as keyboard activity, even slow
motion below a controller's drift threshold. A new pad button press, pressure
increase or deliberate stick deflection selects the pad. Held controls,
key autorepeat, releases and small stick jitter do not steal the selection.
If both devices produce new input in one poll, keyboard input wins. A pad whose name says
Sony, DualShock, DualSense, PlayStation, PS4 or PS5 is a PlayStation pad; the
Apple TV remote and a Steam Controller (by their names) get their own sets;
any other pad gets the 360 set.

| `icons` | Shows |
|---|---|
| `auto` | the last-used device's set. The first poll identifies an attached pad; otherwise keyboard. Before discovery it prefers an available keyboard. Disconnecting the pad returns to keyboard. |
| `xbox` | retail; nothing is installed |
| `xbox360`, `playstation`, `keyboard`, `remote`, `steam` | that set, always |

## Limits and open questions

- Automatic physical-pad discovery needs the combined player-one pad, as
  used by `h2 play`. With the keyboard off there is no pad-kind tracking;
  use a fixed set. Headless `h2 run` uses its default keyboard bindings,
  including for forced keyboard icons. Pads on ports two to four do not
  change the icons.
- The pad's kind comes from its name only. A pad that reports a generic name
  gets the 360 set; pass `--icons playstation` then.
- Text that the game measured before a switch is not measured again until the
  game lays it out again. Retained layouts may keep earlier advances until
  refreshed.
  Live switching on every screen has not been verified.
- The hooks add a host call to every glyph lookup. The cost was not measured;
  `icons = xbox` installs nothing.
- U+E12C to U+E130 (more stick pictures) are not identified and not replaced.
- The hook addresses are for the 1.0 image; a title update that moves the font
  code needs them found again ([title-updates.md](../runtime/title-updates.md)).
- Keyboard labels are English key names. A long label is cut at 12 characters.
- All 32 fonts, including the Asian fonts, were patched and fully decoded
  offline; every original glyph's pixels were checked unchanged. Asian
  language screens have not been run.

## Tests

`cargo test -p font -p vfs -p input`, and `cargo test -p boot --lib icons`.
All use synthetic fonts and in-memory files.

- `font::encode`: every byte kind, run splitting at 63, the eight run alphas,
  literals, and a pseudo-random round trip through the decoder.
- `font::patch`: in-place rewrite of an owned record, a new record for a
  shared or unmapped unit, record 0 untouched, other glyphs, metrics and
  kerning unchanged, header totals equal to the file, bad sizes rejected.
- `font::icons`: the rasteriser (coverage, symmetry, area of a disc, outline
  width, blending, pixel conversion); the label set covers every key name
  `crates/platform/input` accepts; every icon at heights 9 to 24 fits the glyph limits
  and survives the compressor; the bumper pair is white at run levels with the
  right side lit; face balls have the right colours; key caps are light with
  dark letters and follow the labels; each set's code points are distinct; a
  patched font validates and still draws the retail glyphs.
- `vfs::rewrite`: rewritten size in `stat` and listings, reads at and past the
  end, pass-through for other files, a declined file asked once.
- `input::active` and the fallback pad: fresh keyboard/pad activity, slow
  mouse motion, held controls, autorepeat, pad kind by name and drift ignored.
- `boot::icons`: labels from the default and from custom bindings; `auto`
  follows the device; a fixed set ignores it; only font files are rewritten.
  Synthetic generated handlers verify both hook argument offsets, preserve
  the high word and neighbouring arguments, and execute the original handler.

## Offline previews

`h2 font` uses the same binding labels, patcher and substitution rules:

```sh
h2 font disc.iso conduit-16 '<button code points>' --icons playstation --glyph-colors --out "$TMPDIR/buttons.png"
h2 font disc.iso conduit-16 '<button code points>' --icons keyboard --bindings keys.toml --glyph-colors --out "$TMPDIR/keys.png"
```

Supply actual U+E100..E111 characters as the text. Omit `--icons` for the
original glyphs. Previewing requires a fixed set: `auto` needs live input.
`--all` validates original fonts and cannot be combined with preview options.
Output stays outside the repository, as do all visual comparisons.

## Not verified

The synthetic tests do not need disc data. The offline all-font audit and
visual comparison read the user's disc at runtime and commit no pixels.
A run is needed to show that the game's own text path reaches the hooks,
accepts the longer font files and displays the new records; CPU font renders
and synthetic dispatch tests cannot establish that integration.
