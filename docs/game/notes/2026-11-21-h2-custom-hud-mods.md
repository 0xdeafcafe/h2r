# How Halo 2 mods did custom HUD work: research

Evidence for [hud-kit.md](../hud-kit.md), "The charge meter". Nothing here states what the code does; the design page does.

Newest first.

## Community HUD practice and the `nhdt` meter

Read 2026-11-21 at `77c9f8f0b`, from public pages (fetched with a web
search and page fetches; no game run, no disc read). To reproduce, open the
links. Forum pages that refused a fetch are listed under "Not read"; nothing
from them is used.

Sources and their standing:

| Source | Used for | Standing |
|---|---|---|
| Assembly's plugins, [`Halo2/nhdt.xml`](https://raw.githubusercontent.com/XboxChaos/Assembly/dev/src/Assembly/Plugins/Halo2/nhdt.xml), [`Halo2MCC/nhdt.xml`](https://raw.githubusercontent.com/XboxChaos/Assembly/dev/src/Assembly/Plugins/Halo2MCC/nhdt.xml), [`Halo2/shad.xml`](https://raw.githubusercontent.com/XboxChaos/Assembly/dev/src/Assembly/Plugins/Halo2/shad.xml), [`Halo2Xbox/hudg.xml`](https://raw.githubusercontent.com/XboxChaos/Assembly/dev/src/Assembly/Plugins/Halo2Xbox/hudg.xml) | field, flag and enum names | allowed ([tag-field-names.md](../../editor/notes/tag-field-names.md)) |
| c20, the Reclaimers Library: [H2 scripting](https://c20.reclaimers.net/h2/engine/scripting/), [H1 `weapon_hud_interface`](https://c20.reclaimers.net/h1/tags/weapon_hud_interface/), [H1 `meter`](https://c20.reclaimers.net/h1/tags/meter/), [H2EK](https://c20.reclaimers.net/h2/tools/h2-ek/) | script commands, the Halo 1 meter fields | community documentation of the released kits |
| Microsoft's [Halo 2 HaloScript reference](https://learn.microsoft.com/en-us/halo-master-chief-collection/h2/haloscript/haloscripthome) | script commands | official |
| [H2Codez](https://github.com/Project-Cartographer/H2Codez) README | what the kit patch adds | read for scope only |
| Project Cartographer's `new_hud_definitions.h` and `new_hud_draw.cpp` | nothing | provenance of its engine names is unclear; listed, not relied on |

What was found:

1. **There is no meter widget.** `nhdt` has bitmap widgets, text widgets
   and screen effect widgets, and no meter kind. A meter is a bitmap
   widget whose shader fills the art by an input. The widget carries no
   colour and no fill field: four input bytes, eight state words, three
   cutoff bytes, an anchor, the bitmap, the shader, a sequence and an
   offset per screen mode, an effects block and a special type (Assembly
   `Halo2/nhdt.xml`; there is no `Halo2Xbox/nhdt.xml`, the `Halo2` plugin
   is the one that covers it). This matches `crates/formats/tags/src/defs/nhdt.txt`.
2. **Inputs are a fixed list.** Assembly names them: basic (zero, one,
   time, global HUD fade), unit (shield, body, autoaimed, no grenades,
   frag and plasma counts, time on depleted shield, zoom fraction, camo
   value), parent (shield, body), weapon (clip ammo, heat, battery, total
   ammo, barrel spin, overheated, clip ammo fraction, time on overheat,
   battery fraction, locking fraction) and game engine (user score
   fraction, other user score fraction, user winning, bomb arming
   amount). There is no free input a mod can feed, and no vehicle or
   boost input. Bomb arming amount is Halo 2's own "progress while you
   hold" value.
3. **Special types take the widget over.** Assembly lists unspecial, two
   scoreboard emblems, two score meters, unit shield meter, motion sensor
   and territory meter. A widget with a special type gets its inputs, and
   for the shield its colours, from engine code; a plain meter is the
   unspecial kind with a meter shader.
4. **The fill and the colours are the shader's.** Assembly's `shad`
   plugin gives a shader a template, parameters (bitmap, value, colour,
   switch) and animation properties that name an input and a range.
   Halo 1 kept these on the meter element itself (minimum and maximum
   colour, flash colour, empty colour, an alpha multiplier described as
   "the alpha mask difference between successive rounds", c20). Halo 2
   moved them into the shader, so modders recolour a meter by editing or
   swapping its shader, not the widget (inferred from the two plugin
   layouts; no Halo 2 page states it).
5. **Mods edit, swap and reposition; none adds a value.** What the public
   record shows is recolouring, moving and swapping stock widgets and
   their shaders with a tag editor. No documented Halo 2 mod was found
   that drives a meter from a value of its own (a sprint, stamina or
   speed meter), and none that reuses the battery, heat or zoom meter for
   one. That is a negative result of a search, not proof.
6. **Scripts cannot feed a meter.** The script HUD commands show, hide and
   blink stock parts, set the training text, the objective and help text,
   a cinematic title, a timer and navpoints on flags and objects (c20,
   Microsoft). Halo 2 has no `chud_*` commands. The scripted timer and
   the training text are the only script-fed readouts.
7. **A number readout is a text widget.** With "string is a number" the
   text widget prints an input as digits, forced to two or three digits
   by two more flags (Assembly).
8. **MCC added anchors.** The MCC plugin has four more anchors (top,
   bottom, left and right centre). They do not exist on the Xbox, so a
   kit element cannot use them.
9. **Waypoints are `hudg`, not `nhdt`.** Assembly's Xbox `hudg` plugin has
   the waypoint insets, an arrow bitmap and a waypoint arrows block with
   on-screen, off-screen and occluded sequences, which is what
   [hud-kit.md](../hud-kit.md), "The navpoint", reads.

What it means for the kit:

- A meter of ours has to be what the kit already does: a stock meter's
  bitmap and shader in an unspecial bitmap widget, with the fill answered
  at the input point. The community never had a hook, which is why no mod
  shows a custom value.
- The stock art to look at first for a "hold to charge" meter is the
  widget that reads bomb arming amount. Which `nhdt` holds it, and what
  its art is, was not found in any source. It is a disc check.
- The centre anchors, a script-fed value and a per-widget colour are not
  available.

Not read (a 403 or a timeout): the Remnantmods HUD threads
([t=642](https://remnantmods.com/forums/viewtopic.php?t=642),
[t=834](https://remnantmods.com/forums/viewtopic.php?t=834)), the
halo2.online Assembly HUD colour thread and the Nexus Mods HUD pages. Search
snippets of them describe the shield as two widgets (a mask and a meter
whose bitmap carries the fill gradient) and name overlay functions of the
stock meter shaders (gradient minimum, gradient maximum, empty). Those
agree with [health.md](../../halo1/health.md) but are not evidence here.

Not relied on: Assembly's plugins for pre-release builds (not opened),
threads about pre-release HUDs, and Project Cartographer's engine-named
sources. The two things Cartographer would add (the four input floats and
the shield meter's special path) are already read from the retail XBE in
[hud-kit.md](../hud-kit.md), "Widgets".
