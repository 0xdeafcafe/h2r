# HUD: measurements

Evidence for [../hud.md](../hud.md).

## 2026-11-20: the weapon HUD census on the discs

Commit: `237749036`. Machine: the owner's Halo 1 disc. Method: a
read-only census of every `wphi` chain of the ten campaign and thirteen
multiplayer maps.

1,497 authored weapon draw entries in 360 interfaces.

## 2026-11-19: the weapon HUDs checked live on a converter-v83 map

Commit: the map was built by converter v83; the threshold word the build
mistyped was fixed for v84 in `40b48dae5`. Machine: the hud serve,
driven with `h2 agent edit get` and `edit.set`.

On the v83 map the from-scratch build had typed the warning widget's
`+0x18` threshold word as a raw-resource pointer and written a file
offset there (`00 da 22 01`: rounds loaded below 218, so "Reload"
always). Writing each weapon's thresholds back with `edit.set`
(`bitmap_widgets[3].raw_18`, assault rifle `0x00010b00`) hid "Reload"
at 60 loaded, showed it at 5 loaded with 180 in reserve, and showed "No
ammo" alone at nothing loaded and nothing in reserve. Every other
weapon's HUD, drawn on the held assault rifle by pointing its `hud`
reference at it, had its panel, count, icon and meter where Halo 1 puts
them and no widget over another; only the always-on "Reload" overlapped
the crosshair.
