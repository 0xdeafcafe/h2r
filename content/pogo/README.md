# The Pogo Stick

Three looks of one stick, the weapon a Superbounce player carries
(docs/game/superbounce.md): `unsc`, a stamped olive field spring with a coil
and a chromed piston; `covenant`, a curved shell over a plasma column and
cushion; `forerunner`, a six-sided alloy strut over a core of light and three
rings. Each look is a model source, `models/<look>.yaml`, and a texture atlas,
`models/<look>.png`.

**Original.** The shapes and the paint are ours, drawn for this project.
Nothing here is taken from a Halo game, and there are no logos, emblems or
lettering from one: the only marks are stencil numerals from the painter's
own digit set. No file here holds bytes from the disc. What a map must lend
the stick (a shader template, a material, the weapon class the players hold)
is read from the owner's map when the kit is built, and is not stored.

## How it is built

This folder is the `pogo` package (`package.toml`, baked into `h2` by
`crates/apps/boot/build.rs`). Per look, a `model` item
(`models/<look>.yaml`, with `models/<look>.png` named as a `data` item) builds
`bitm`, `shad`, `mode` and `hlmt` (`crates/tools/workspace/src/model.rs`,
over `crates/tools/content-kit`), and a `tag` item (`tags/pogo_<look>.weap.yaml`)
builds the `weap`. `sounds/` holds the three clips the game plays
(`sound` items) and `data/poses.yaml` the rider's pose set. A multiplayer map
gets them when the game reads it (`workspace::PackageRewrite`). `h2 pogo models --out DIR` writes the three meshes as OBJ files to
look at.

A model source is a list of parts, each one of three primitives on one of
two nodes (`frame`, and `piston`, which slides up into the frame):

| Primitive | What it is | Atlas mapping |
| --- | --- | --- |
| `lathe` | a profile of (radius, distance) points turned about an axis | u around, v along the profile |
| `sweep` | a circle or polygon carried along a path | u along the path, v around the section |
| `box` | a box, its edges bevelled | the whole region on each face |

Units are world units (1 = 3.048 m): +X forward, +Y left, +Z up, the origin
the right grip. Every look has the same five markers at the same places
(`grip_right`, `grip_left`, `peg_right`, `peg_left`, `foot`), so a rider is
posed the same on each. A look stays under 2000 triangles.

The atlases are painted by `scripts/pogo-atlas.py`, which uses the Python
standard library, reads nothing and paints the same bytes every run:

```sh
python3 -I scripts/pogo-atlas.py
```

`--check` exits 1 when a PNG differs from what the script paints.

## The atlas

512x256, opaque, the same regions on all three (pixels, x0 y0 x1 y1):

| Region | Rectangle | Holds |
| --- | --- | --- |
| `body` | 0 0 256 128 | the main shell |
| `body_dark` | 256 0 384 128 | the lower sleeve and the dark frame parts |
| `piston` | 384 0 512 128 | the piston rod or light column |
| `grip` | 0 128 128 192 | the two hand grips |
| `peg` | 0 192 128 256 | the foot pegs' tread |
| `foot` | 128 128 256 256 | the foot |
| `accent` | 256 128 384 256 | painted panels, numerals, light seams |
| `glow` | 384 128 512 256 | the coil, the plasma, the light rings |

A part names its region; its texture coordinates stay two texels inside it.

**Shading.** One opaque shader draws the whole stick with the atlas as its
base map. Nothing glows on its own: the plasma, the light core and the
seams are painted bright in the atlas.

The PNGs are stored in Git LFS (`.gitattributes`).

## Sounds

`compress.wav`, `spring.wav` and `settle.wav`: the piston going in on a
touchdown, springing out on a bounce, and easing out after a landing with
no bounce (docs/game/pogo-stick.md, "Sounds"). Each is 44.1 kHz mono 16-bit
PCM, synthesised by `scripts/pogo-sounds.py` from sine partials and
filtered noise: no recording or sample goes in.

```sh
python3 -I scripts/pogo-sounds.py
```

`--check` exits 1 when a WAV differs from what the script writes. The WAVs
are stored in Git LFS (`.gitattributes`).

## Poses

`poses.yaml` holds the rider's poses: the bone names by role with their
aliases, the mounted pose's numbers and each trick's
(docs/game/pogo-stick.md, "The pose"). They are hand-keyed and have not
been seen in a game; its header documents every field.

`kit.yaml` is the item manifest (`crates/tools/content-kit`, `atlas`): the atlas size and the regions a part names, and the stick's kit version, tag paths, material, region, bounding and physics spheres, rig points, node and marker names, and the weapon class it is held as. The kit reads them; it holds no stick-specific value of its own.
