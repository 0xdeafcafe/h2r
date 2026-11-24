# game-markers

The marker set Forge and Studio draw for the parts of a game type: player
spawns, flag and bomb places, hills, territories, teleporters, item
spawns, trigger volumes and the labels a script reads. The design is
[docs/game/gametype-markers.md](../../../docs/game/gametype-markers.md).

## Where it sits

The `formats` domain, with no dependencies: plain numbers in, lines and
triangles out. The drawers (Studio's viewport, the game's world-line
pass) and the document (`forge-doc`) depend on it.

## What it has

- `Marker { kind, team }` and `Marker::mesh(&Palette)`: a `Mesh` of
  coloured lines and translucent triangles in the marker's own space
  (x forward, y left, z up, world units). `Mesh::placed(&Place)` puts it
  in the world.
- `Kind`: every marker, with the parameters its shape takes (an `Area`
  for a hill, a territory or a site; a `Solid` for a volume).
  `Kind::of_label` names the marker of a label's text.
- `Area`: the ground a hill or a site covers, a radius or a path of
  points round the centre marker, with its heights. `Area::validate`
  refuses what the tools must not store, `Area::contains` is the test a
  zone makes, `Area::of_corners` is the area of a map's hill corners.
- `shapes`: the primitives the markers are made of, for a drawer's own
  handles.

Every shape is made here from numbers of ours. Nothing is read from a
disc.
