# Extending the on-screen keyboard's character set (design)

Status: research and design only. Nothing here is built.

Halo 2's virtual keyboard types names: the player profile, game variants,
playlists, saved films, squad and clan names, messages and gamertags. Its
character set is small, and names in the wild use more. This document says
what the keyboard does today, what players and modders use, and how we could
add characters without breaking rendering, saves or the network.

No game data is in this document: no glyph bitmaps, no tables copied from the
game, no retail strings beyond short descriptions of what classes of character
exist. Community sources are cited, not copied.

## Evidence labels

- **Local**: read from the retail disc or XBE with this repo's tools
  (`h2 ui`, `h2 strings`, `h2 font`), or already in another doc here.
- **Community**: a public source. Many describe the Xbox profile editing scene
  or Halo 2 for Windows Vista (H2V), which is a different build. Marked where
  the source is a search-result summary because the page itself could not be
  fetched (several of these forums return 403 or a bot wall).
- **Speculation**: my reasoning, to be tested.

## How the keyboard works

### The screen (Local)

The keyboard is one `wgit` screen with one pane ([ui.md](ui.md); find it with
`h2 tags <map> wgit`, and print it with `h2 ui`). Its structure:

- 45 distinct buttons: ten digits, 26 letters, backspace, space, left, right,
  done, shift, caps lock, "accents" and "symbols". The accents and symbols
  buttons each have a second, greyed-out copy at the same position, so the
  screen lists 47 button entries.
- Two text blocks (the typed text and a help line), an input-field bitmap, a
  header string and a dialog backdrop.
- The buttons are placed on a grid with ten columns at a 35-unit pitch: the
  digit row, then the letters in rows of ten, ten and six, with the space,
  arrow and function buttons below and to the right. Every key is one widget
  bound to a label string ID.
- The screen has one set of widgets and changes the labels. There is no
  separate screen per page. The buttons keep their positions and the screen's
  code swaps the string each button shows.

Every label is a string in the screen's own `unic`, one set per language slot
([localization.md](localization.md)). The English set has 213 strings:

- a base page: digits and lowercase letters;
- a **shift** page: ten punctuation marks on the digit row and uppercase
  letters;
- an **accents** page: digits and 26 accented lowercase letters;
- a **shift + accents** page: digits and 26 accented uppercase letters;
- a **symbols** page: on the digit row, four two-character emoticons and four
  single-character dingbats; then 26 punctuation, currency and bracket
  characters;
- function labels (done, shift, space, backspace, arrows) that start with a
  private-use icon glyph in the E1xx block, followed by plain text;
- a header and a help line per entry context (profile name, variant name,
  playlist, film, gamertag, message, squad name, clan name, clan description,
  clan motto, clan-mate gamertag).

There is no shift variant of the symbols page.

So **the keyboard layout is data**: strings in a `unic`, indexed by the button.
The mapping from button and page to string ID, and the typing logic, are code.
Nothing in the tag says which page a button uses or how long the output can be.

### Length limits (Local)

The help strings say that profile, game variant, playlist and film names may
be up to 16 characters, and that clan names may be up to 15 and are limited to
letters, numbers and spaces. The squad (game session) name is described as what
other players see over the network. Presumably the code enforces the limits;
which function does, and whether it counts UTF-16 units, is not traced.
Messages and the gamertag entries state no limit in their help text.

### What the typed text is (Local, from [localization.md](localization.md))

The game works in 16-bit text units. UI strings are decoded from UTF-8-like
bytes into 16-bit units, and four-byte sequences are narrowed to their low 16
bits instead of becoming surrogate pairs. So the engine's text is effectively
the Basic Multilingual Plane. Anything beyond U+FFFF cannot be represented, and
an emoji from the astral planes would arrive as garbage.

The UI wrapper copies at most 255 units and terminates the buffer.

### Evidence from the Windows build (Community)

Project Cartographer's source declares the H2V keyboard's shape, and it
matches the Xbox screen:

- 47 button types, in the order digits, letters, backspace, space, left, right,
  done, shift, caps lock, accents (two), symbols (two):
  [`screen_virtual_keyboard.h`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/interface/screens/screen_virtual_keyboard.h).
- A fixed four rows by ten columns, a 256-unit wide-character buffer and a
  "context" value that says what is being entered (profile name, variant name,
  playlist, film, message, squad, search description).
- The accents and symbols pages are gated by context: the Cartographer code
  patches the comparison that decides which contexts allow them. That explains
  the greyed-out buttons in the Xbox tag, which exist in both builds.
  See [`screen_virtual_keyboard.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/interface/screens/screen_virtual_keyboard.cpp).
  (Which Xbox contexts disable them is not traced here.)
- The H2V input layer also accepts physical keyboard characters. A
  `WM_CHAR` path builds a keystroke with an `ascii_code` and a `utf16_code`
  field
  ([`input_windows.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/input/input_windows.cpp)).
  Whether the Xbox build has the same field is unknown (Speculation: probably
  not for the retail console build, which only knew USB gamepads, but it is the
  first thing to trace).
- Release 0.7.2.4 lists "Improved Keyboard localization"
  ([changelog](https://www.halo2.online/threads/project-cartographer-0-7-2-4-update-changelog.15785/));
  earlier releases fixed a Spanish layout key opening the advanced settings
  menu. The H2V FAQ says to use the on-screen Shift to reach `@`. The
  changelog entries are not about the Xbox set and give no characters.

These are H2V addresses and layouts. They tell us the shape, not where to hook
the Xbox executable.

## Which characters it allows now

All Local, from the English `unic` of the keyboard screen:

- Letters a to z and A to Z, digits 0 to 9, space.
- Printable ASCII punctuation: all of it **except** backtick, pipe and tilde.
  The pipe is the character the engine's text scanner treats as the start of an
  inline markup escape ([fonts.md](fonts.md)), so leaving it out looks
  deliberate (Speculation). Backtick and tilde are probably plain omissions.
- Latin-1 accented letters on the accents page, lower and upper case: grave,
  acute, circumflex and diaeresis forms of a, e, i, o and u, plus cedilla,
  eszett, n with tilde, y with diaeresis (lower and upper) and the ae ligature.
  Not present: a with ring, o with stroke, the oe ligature, thorn, eth, and
  letters from Latin Extended-A (Polish, Czech, Turkish and so on).
- Currency: pound, euro, yen. Inverted question and exclamation marks. Angle
  quotation marks.
- Four emoticons (typed as two characters each) and four dingbats (star, flag,
  check mark and ballot x) from the BMP.
- Nothing from Greek, Cyrillic, CJK or Hangul, even though the CJK builds ship
  large fonts ([fonts.md](fonts.md)). Whether the non-English `unic` sets have
  different layouts was not checked; only English was read.

### What can be typed from outside the keyboard

Not at all in the base game. The sources below, all Community, describe editing
the saved file, which the keyboard never produces.

## What the community uses

Evidence is thin and most of it is about the Xbox save editing scene. I could
not fetch most of the forum pages (403, bot walls); the facts below come from
search-result summaries and are marked.

### Button and weapon icons in names (Community)

- Modders put picture icons (stars, an energy sword and so on) in profile and
  game variant names by editing the save with tools such as Sav3r, then
  re-signing it. The workflow: pull the `profile` file off the console, open it
  in the tool, double-click icons from its list to add them to the name, save,
  re-sign, copy it back
  ([Sav3r how-to](https://www.se7ensins.com/forums/threads/halo-2-using-sav3r-to-edit-your-profile-gametypes-name-pics.62573/),
  [HaloMods "Symbols In User Profile"](https://www.halomods.info/viewtopic.php?p=618002),
  [Sav3r release thread](https://www.halomods.info/viewtopic.php?t=18064&start=180),
  [TeamXLink "halo 2 names"](https://teamxlink.co.uk/forum/viewtopic.php?t=19138&highlight=)).
  The tool names the icons by label. A user reports a "damaged" error after
  inserting too many.
- The same trick works for the game variant title. A tutorial titled for Halo 3
  symbols in a variant title says to hex edit a byte of the title and warns
  that not every symbol works and that too many symbols corrupt the save
  ([Se7enSins](https://www.se7ensins.com/forums/threads/halo-3-symbols-in-game-variant-title.93661/)).
  Which game that one covers is unclear from the title. The summary mentions
  recommending a name with filler letters first so there is room.
- Manual hex editing needs the file re-signed. A hacks book gives the shape of a
  re-signer's per-title configuration for Halo 2 (the signature at the end of
  the file, covering what precedes it)
  ([Hack 89](https://flylib.com/books/en/2.66.1.127/1/)). We do not need any
  of that: the game itself writes and signs the files in our runtime.
- The icons are ordinary 16-bit characters in an unused range: the "Halo 2
  Font" thread on Remnantmods says the button images are valid characters in a
  mostly unused part of Unicode, that they are absent unless a second font is
  present, and lists the button, d-pad, trigger, thumbstick and weapon
  pictures by code
  ([thread](https://www.remnantmods.com/forums/viewtopic.php?t=2106),
  [page 2](https://remnantmods.com/forums/viewtopic.php?t=2106&start=25)). It
  also describes adding glyphs by appending data at the end of the font file
  and repointing a blank index entry. Its code table is not reproduced here.
  Our own finding agrees with its shape: the font files map the private-use
  range with a glyph record per icon ([fonts.md](fonts.md): the scanner's icon
  range, colour literals in the icon records). How the thread's single-byte
  codes relate to our code points is not established. Most plausible is that
  they are the low byte of a U+E0xx or U+E1xx code point (Speculation).
- The Baboon tag editor's pull request that adds string-list editing notes that
  Halo 2 has button glyphs and that no tool of theirs knew their names
  ([PR 111](https://github.com/Zoephie/Baboon/pull/111)). So no public name table
  for the Halo 2 icon range was found.

### Accented and symbol characters (Community)

- Halo PC (the first game) players used Windows alt codes and Character Map to
  get symbols and accents into names
  ([a 2006 Bungie forum post](https://www.bungie.net/en/Forums/Post/360068),
  [a GameTracker thread](https://www.gametracker.com/games/halo/forum.php?thread=130570),
  which also gives that game's name limit as 11). That is Halo CE on PC, which
  has no virtual keyboard. It shows demand, not Halo 2 behaviour.
- For H2V, a forum question asks how to put medals or symbols in the profile
  name, and a reply says hex editing the profile is needed
  ([iBotModz](https://ibotmodz.net/topic/20373-halo-2-for-windows-vista-how-do-i-put-medals-or-symbols-in-my-profile-name/),
  from a search summary; the page itself was blocked).
- Modern Xbox gamertag symbol lists
  ([for example](https://fsymbols.com/emoji/collections/xbox-gamertag-symbols/))
  are about a different system with its own rules and do not apply.

### Summary of demand (partly Speculation)

What people want, ranked by how often it came up and how cheap it is:

1. Button and weapon icons, in profile and variant names. Common, game-specific,
   already in the fonts.
2. Accents and a few letters the keyboard lacks (a with ring, o with stroke,
   the oe ligature, thorn, eth). Cheap: the retail fonts already draw them (see
   below).
3. Typographic and technical symbols: trademark, copyright, registered, degree,
   multiplication sign, arrows, math symbols, circled letters. Needs glyph
   work for many.
4. Non-Latin alphabets: Cyrillic, Greek, CJK. Large; the CJK fonts exist but
   only in the matching language builds.

I found no usable ranking from a survey. The ordering is my judgement.

## Rendering: does the font draw them?

### What the retail fonts cover (Local, sampled)

[fonts.md](fonts.md) describes the format: a 65,536-entry character map per
font, indexed by UTF-16 unit. An entry of zero means "no glyph". I rendered a
test string with `h2 font` using two Latin fonts. In both of them:

- present: the Latin-1 letters the keyboard lacks (a with ring, o with stroke,
  the oe ligature, thorn, eth), euro, trademark, copyright, registered, degree,
  multiplication sign, some Latin Extended-A (a few letters with caron and
  stroke), and the keyboard's four dingbats;
- drawn as a box (no glyph): Cyrillic, Greek, arrows, and at least one Latin
  Extended-A letter (D with stroke).

That is a sample of two fonts, one test string, by eye. It is enough to say
that **many extra characters need only a keyboard entry, not a new glyph**.

The Latin fonts are 11 engine slots over eight files, and there is a different
file set per Asian language ([fonts.md](fonts.md)). Coverage differs per file
(270 to 1,234 glyphs), and a keyboard entry is useless on a screen whose font
file lacks the glyph. A proper audit is needed before promising anything.

**Audit tool (new).** `h2 font --coverage <iso> <charset-file>` prints, per font
file, which code points of a set have a glyph (map entry nonzero and a record
with a nonempty bitmap). It writes no glyph data. The charset is a list of code
points kept in the repo; the output goes to the terminal. Tests use a synthetic
font.

### Which font draws which field (Not traced)

[fonts.md](fonts.md) lists, as still open, which engine font slot serves which
UI element. The keyboard's key labels use one font index and the typed text
another, and every later place the name appears (lobby, scoreboard, medal
text, saved-film list) may use a third. A name is only as good as the
**narrowest** font among all those places. The audit has to be run against
every font that can show a name, which first needs that mapping.

### Adding a glyph (Design)

The glyph file is read by the game through file I/O, which means our VFS sees
every read ([storage.md](storage.md)). That gives a clean place to add glyphs
without touching guest code or committing glyph data:

1. At first run, generate an **overlay font file** for each Latin font in the
   user's cache. It is the user's own font, byte for byte, plus appended
   glyph records and pixel data for the added code points, with the matching
   character-map entries repointed.
2. Serve the overlay file in place of the original from the VFS when the game
   opens that name. The retail bytes on disc are unchanged and the repo has
   none.
3. Pixel data is produced at run time by rasterising from a font that the
   project can legally use (an OFL font such as a Noto or Unifont family; a
   licence decision that needs recording before choosing one), then encoded in
   the engine's compressed 4444 format using the decoder rules in
   [fonts.md](fonts.md). Two options for appearance: draw the new glyphs in a
   neutral font and accept the mismatch, or (for accented letters) compose the
   accent from the user's own retail base letter and a small mark.
   Composition keeps the look but is more work; both are generated in the
   user's cache.

Constraints from [fonts.md](fonts.md) that this design must respect:

- records are 16 bytes; the engine clamps glyph width and height to 128 and the
  compressed size to 1 to 4,096 bytes;
- the header's glyph count, total data size and file length are internally
  consistent on every retail font; the engine reads only the first 0x1c4 header
  bytes and addresses records by index, so appended records past the current
  end should work, but whether anything checks the count is not traced
  (Speculation: nothing does; the Remnantmods thread describes exactly this
  technique working);
- Latin placeholder records exist in some fonts (zero width and size) and can
  be reused, but not every font has them;
- kerning applies only to units 1 to 255; added characters get none;
- the glyph cache and its I/O buffer have a size we have not measured. A large
  added set in one font could evict more often. Measure on the draw path.

### Private-use icons

The icons are already in the fonts. To type them, the keyboard needs entries
(or an input path) that produce the right code points, and the game must be
told their names. Because no public name table exists (Community: the Baboon
note above), we would build our own picture list by rendering each private-use
glyph to a screenshot and labelling it by eye, as a local development step.
The result is a list of code points with our own names, which is fine to
commit; the glyph pictures are not.

## How we could add characters

### Option 1: more keyboard pages (native)

Add keys, or a third page, to the retail keyboard screen.

- Data side: add string IDs to the keyboard's `unic` (for each of nine language
  slots) and a way to point a button at them. Adding a button means adding to
  the widget tree; the screen has a fixed set of 47 button entries and a pane
  with a fixed layout. The practical version is to **re-label existing keys**:
  the symbols page has 26 plus 8 slots and the screen swaps labels already.
  Replacing the lower-value entries on the symbols page (some emoticons, some
  rare punctuation) with more useful ones costs no layout change.
- Code side: the page selection and typing logic are code. A shift variant of
  the symbols page does not exist, so a second symbols page would need a new
  page state or a repurposed key, and that needs a hook in the screen's
  event-handling code, as Cartographer does on H2V. We have not located that
  code on Xbox.
- Looks native, works with a pad, is localizable per language, but is the most
  reverse engineering and gives a few dozen extra characters at most.

### Option 2: host text entry

Let the player type with a real keyboard or the OS input method, and write the
result into the guest's buffer.

- The host side is easy. The shell already receives key events; add a
  text-input path (winit gives committed text and IME composition), the same
  one a settings-menu text field needs ([settings-menu.md](settings-menu.md)).
- Two ways to hand the text to the game:
  a. **Fill the guest buffer.** The keyboard screen owns a wide-character buffer
     (256 units in the Windows build; Xbox size not traced), a length and the
     text widget. On a host commit, append units to the buffer, update the
     length, refresh the typed-text widget, cap at the context's limit. The hook
     point is the screen's key handler, and it must keep the buffer's
     invariants. The input is a short UTF-16 string, so this is a small
     state-only write.
  b. **Replace the screen.** Detect that the keyboard screen opened (screen ID
     at the widget, [ui.md](ui.md)), pause the guest, show a host text field,
     write the final string into the buffer and synthesise "done". Easier and
     more robust to engine details, but it bypasses the game's own screen, and
     the player sees a different dialog.
- Either way, this removes the layout problem: any character the keyboard
  cannot type can be entered, and **the font becomes the limit**. A host path
  needs a filter so that a unit with no glyph in the relevant fonts is
  refused or replaced before it enters the buffer, or the player gets blank
  or boxed names (below).
- It does not look native and needs a pad-free alternative (the pad user still
  has the retail keyboard), but it is the cheapest to build and the only way to
  reach non-Latin text.

### Recommended order

1. Run the coverage audit (needs the font-slot trace for names).
2. Option 1, **relabel only**, for characters the fonts already have (a with
   ring, o with stroke, oe, thorn, eth, trademark, copyright, degree and
   similar). It needs the keyboard-screen trace, so it follows that discovery.
3. Option 2 with the filter, for anyone with a physical keyboard, limited to
   characters with glyphs.
4. The overlay font (glyph generation) only when 3 shows demand for more.
5. Icons last, because they need names and a picker.

## Making sure added characters render, survive saving and stay safe

### Render

- Filter by coverage at entry time. Every text-entry path uses one function,
  `usable(ch, context)`, driven by the audit's output. A character with no
  glyph in every font that can show that kind of name is refused (host input)
  or never offered (keyboard pages).
- When the filter must accept a character that some font cannot draw, the game
  draws a box or nothing there; it never crashes (Local: [fonts.md](fonts.md),
  zero map entry draws record zero). A name with one missing glyph is ugly,
  not broken.
- Test: render each added code point through the font crate for each affected
  font and assert a nonempty bitmap. Synthetic fonts for the unit tests, the
  disc for the ignored audit.

### Saving

- The profile and variant files are signed by the game's own
  `XCalculateSignature` calls, which our runtime provides
  ([xapi.md](xapi.md), [checkpoints.md](checkpoints.md)). A name the game writes
  is signed by the game, so no re-signing tool is involved; that is the
  difference from the Sav3r workflow above.
- Native save names (`SaveMeta.xbx`) are UTF-16LE with a byte-order mark
  ([checkpoints.md](checkpoints.md), [storage.md](storage.md)). BMP code points
  survive. Astral code points do not exist in the engine's text (above). A host
  input path must reject surrogate pairs.
- The community reports file corruption when too many symbols are packed
  (Community, Se7enSins above). That is likely a length-in-bytes versus
  length-in-units mismatch from hex editing; with the game doing the writing,
  the risk is in our own length cap. Cap in units, as the game does, using
  the context's limit.
- The name travels through the host file system as bytes. The VFS has no
  character-set rules beyond path syntax ([storage.md](storage.md)); names inside
  the files are payload, not paths. Native save directories are named by a
  hash of the name (a 12-hex-digit directory, [storage.md](storage.md)), so
  non-ASCII names do not become host paths.
- Test: a synthetic save round trip through the VFS with a name containing each
  added class (accented, symbol, private-use), read back unchanged.

### Network and other formats

- Not traced. We do not know how Halo 2 serialises a name on the wire, nor
  whether the session, lobby and squad-name structures use fixed-width 16-bit
  fields, 8-bit fields or a conversion that drops characters. The transport
  functions are identified but not audited ([engine-map.md](engine-map.md)),
  and no multiplayer path is exercised yet.
- Real Xbox Live is gone, and Live's own gamertag rules are not ours to extend.
  Gamertag-entry contexts (friend and clan-mate lookup) feed a service and must
  keep the retail character set. Squad names, messages and playlists go to
  other players and are the risky ones.
- Policy until the wire format is traced: allow extended characters only in
  contexts that stay local (profile name, game variant name, saved-film name),
  and keep the retail set for the contexts that are sent (gamertag, message,
  squad, clan; the clan help text already restricts names to letters, numbers
  and spaces). Private-use icons are local-only, since a receiving machine with
  a different font file shows a box.
- Test: a table-driven test of `usable(ch, context)` for each context and
  character class; a trace of what each context does with its buffer once
  multiplayer is reachable.

### Format risks to check

- Checkpoints copy raw game-state memory ([checkpoints.md](checkpoints.md)).
  They hold the name's units as the game stored them, so the same BMP and
  length rules apply, and nothing else changes.
- A different language build or a `unic` with a different layout can place a
  character on a different font. The audit therefore runs per language table,
  not only English.
- Title updates ([title-updates.md](title-updates.md)) may change the keyboard
  screen or the font table. Pages added by hook depend on the 1.0 image; host
  text entry does not.

## Tests

Per the project rule, no game runs for any of this.

1. Font crate: coverage function over synthetic fonts; overlay writer produces
   a file the existing parser accepts and every record decodes to
   `width * height` (the same check as `h2 font --all`).
2. Overlay: the generated glyph for each added code point decodes and
   renders non-empty; the original glyphs are byte-identical.
3. `usable(ch, context)`: table-driven.
4. Host text entry: a fake keyboard screen buffer; commit, cap at the limit,
   reject surrogates, reject uncovered characters, backspace, cursor.
5. VFS round trip for save names.
6. Ignored disc tests: the coverage audit for every font and language;
   checks that the retail keyboard's own characters are all covered (a
   regression guard for the audit itself).

## Open questions

- Which font slots draw names in each place, and each font's coverage of the
  target set.
- The Xbox keyboard screen's key handler, page logic, buffer, and which
  contexts disable accents and symbols.
- Whether the Xbox input layer carries typed characters at all.
- The wire format of names in the session and lobby.
- The relation between the community's icon codes and the font's private-use
  records, and a name list for them.
- Whether non-English keyboard layouts differ.
- Licensing of the font used for generated glyphs.

## Sources

Community, cited and not copied:

- Project Cartographer keyboard and input source:
  <https://github.com/pnill/cartographer>, in particular
  [`screen_virtual_keyboard.h`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/interface/screens/screen_virtual_keyboard.h),
  [`screen_virtual_keyboard.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/interface/screens/screen_virtual_keyboard.cpp),
  [`input_windows.cpp`](https://github.com/pnill/cartographer/blob/master/xlive/Blam/Engine/input/input_windows.cpp);
  [0.7.2.4 changelog](https://www.halo2.online/threads/project-cartographer-0-7-2-4-update-changelog.15785/).
- Xbox profile and variant name editing: the Se7enSins, HaloMods and
  TeamXLink threads linked above; [Hack 89](https://flylib.com/books/en/2.66.1.127/1/).
- Font structure and icon range: the Remnantmods threads
  ([1](https://www.remnantmods.com/forums/viewtopic.php?t=2106),
  [2](https://remnantmods.com/forums/viewtopic.php?t=2106&start=25)).
- Halo PC name tricks: [Bungie forum post](https://www.bungie.net/en/Forums/Post/360068),
  [GameTracker](https://www.gametracker.com/games/halo/forum.php?thread=130570).
- Halo 2 Vista: [iBotModz question](https://ibotmodz.net/topic/20373-halo-2-for-windows-vista-how-do-i-put-medals-or-symbols-in-my-profile-name/).
- Tag editor note on icon names: [Baboon PR 111](https://github.com/Zoephie/Baboon/pull/111).
