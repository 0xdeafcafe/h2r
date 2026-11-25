# Halo 1 HUD: game runs

Evidence for [../hud.md](../hud.md).

## 2026-11-19: the reload warning's thresholds

Image: converter version 83, commit `d278af342`, on the owner's local
`h2 serve`. Assault rifle, 60 loaded, 180 in reserve: the HUD showed
"Reload" on a full magazine. The converter had written the thresholds
right (`00 0b 01 00` at the widget's `+0x18`), but the from-scratch map
build read every tag through the inferred schema, which typed that word
as a raw-resource pointer; it copied "the resource" at file offset
`0x10b00` and wrote the copy's offset there (`00 da 22 01` in the built
map: rounds loaded below 218, so always). Version 84 (`c8248825d`) leaves
the word out of the schema (`tags::schema::infer::ENGINE_DATA`).

The earlier live proof of the gate wrote the thresholds into memory with
`edit.set` after the build, which is why it did not see this.

Method on the version 83 map: write each weapon's thresholds back with
`edit.set` (`bitmap_widgets[3].raw_18`, assault rifle `0x00010b00`). Result:
"Reload" hid at 60 loaded, showed at 5 loaded with 180 in reserve, and
"No ammo" showed alone at nothing loaded and nothing in reserve. Every
other weapon's HUD, drawn on the held assault rifle by pointing its `hud`
reference at it, had its panel, count, icon and meter where Halo 1 puts
them and no widget over another; only the always-on "Reload" overlapped
the crosshair.
