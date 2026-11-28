# The Flood pod

The pod a Flood squad arrives in in Firefight
([docs/game/firefight.md](../../docs/game/firefight.md), "The Flood pod"): a
fleshy seed that falls root first, lands on a bone tip and bursts open. A
ribbed root bulb, a collar where four petals of shell hinge, a lit sac
under them that the squad comes out of, and ribs, pustules and tendrils
down the root. The model source is `flood_pod.yaml` and its texture atlas
is `flood_pod.png`.

**Original.** The shapes and the paint are ours, drawn for this project.
Nothing here is taken from a Halo game, and there are no logos, emblems or
lettering. No file here holds bytes from the disc. What a map must lend
the pod (a shader template, a material) is read from the owner's map when
the kit is built, and is not stored.

## How it is built

`crates/tools/content-kit` builds the pod's `bitm`, `shad`, `mode`,
`phmo`, `hlmt` and `vehi`, all named `objects\vehicles\flood_pod\flood_pod`,
into a multiplayer map. `h2 flood-pod kit MAP --out FILE` writes such a map
to open in Studio, and `h2 flood-pod model --out DIR` writes the mesh as an
OBJ file.

`kit.yaml` is the item manifest (`crates/tools/content-kit`, `atlas`): the atlas size, its regions and the
pod's triangle limit, read by the kit and checked on load.

The source is the Pogo Stick's primitive YAML (`content/pogo/README.md`):
a list of parts, each a `lathe`, a `sweep` or a `box`, on one node. Units
are world units (1 = 3.048 m): +X forward, +Y left, +Z up, the origin the
point of the tip, which is where the pod meets the ground. The pod is about
1.2 units tall and 0.8 across, and stays under 2000 triangles.

| Node | Parent | At | Holds |
| --- | --- | --- | --- |
| `shell` | none | 0 0 0 | the tip, the root, the collar, the sac, the ribs, the pustules, the tendrils |
| `petal_0` | `shell` | 0.345 0 0.54 | the +X petal: a plate and its spine |
| `petal_1` | `shell` | 0 0.345 0.54 | the +Y petal |
| `petal_2` | `shell` | -0.345 0 0.54 | the -X petal |
| `petal_3` | `shell` | 0 -0.345 0.54 | the -Y petal |

The petals are drawn shut. A petal's node is its hinge, on the collar; the
game opens the pod by turning each petal about the hinge's tangent
(`content_kit::flood_pod::RIG`: `hinge_axis`, and `open`, 1.9 radians), which lays
it outward and down.

| Marker | Node | At | For |
| --- | --- | --- | --- |
| `impact` | `shell` | 0 0 0 | the landing effect, and the point put on the ground |
| `squad_exit_0` to `squad_exit_3` | `shell` | 0.75 from the axis on the ground, between two petals | where the squad's members are placed |

`content_kit::flood_pod::source::parse` refuses a source whose nodes or markers are
not these.

The atlas is painted by `scripts/flood-pod-atlas.py`, which uses the Python
standard library, reads nothing and paints the same pixels every run:

```sh
python3 -I scripts/flood-pod-atlas.py
```

`--check` exits 1 when the PNG's pixels differ from what the script paints.
Both print the pixels' hash, which the kit's tests hold
(`crates/tools/content-kit/src/flood_pod/tests.rs`, `ATLAS_PIXELS`): after a change
to the paint, write the PNG and put the new hash there.

## The atlas

512x256, opaque (pixels, x0 y0 x1 y1):

| Region | Rectangle | Holds |
| --- | --- | --- |
| `flesh` | 0 0 256 128 | the root: sallow flesh in swollen rings, veined |
| `petal` | 256 0 512 128 | a petal's plate, hinge to tip: the top half its ridged outer hide, the bottom half its raw inner face |
| `sac` | 0 128 128 256 | the sac and the pustules: a membrane lit from inside |
| `bone` | 128 128 256 256 | the tip and the petals' spines |
| `rib` | 256 128 384 256 | the collar and the ribs: gristle |
| `tendril` | 384 128 512 256 | a tendril, root to end |

A part names its region; its texture coordinates stay two texels inside it.

**Shading.** One opaque shader draws the whole pod with the atlas as its
base map. Nothing glows on its own: the sac is painted bright in the atlas.

The PNG is stored in Git LFS (`.gitattributes`).
