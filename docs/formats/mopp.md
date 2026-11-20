# Havok MOPP code (`sbsp+0x1e0`) and writing it

Status: Built. Code: `crates/formats/mopp`. Switch: none.

A structure BSP carries a Havok MOPP program: bytecode that walks a
bounding-volume tree over the level's collision surfaces and emits the
key of each surface a query may touch. Havok bodies (vehicles, dropped
weapons, grenades, crates) collide with the level only through it. There
is no fallback: a converted level without a real program has no Havok
collision at all. This page is the format as the engine reads it, the
engine's interpreter semantics, and the writer in `crates/formats/mopp`.

Code: `crates/formats/mopp` ([README](../../crates/formats/mopp/README.md)).

Evidence: **E** read from the engine code in the retail XBE (addresses
given); **V** checked on the owner's disc (all 78 structure BSPs of
`disc/halo2.iso`, campaign, multiplayer and the main menu, unless a level
is named). Havok's SDK is proprietary; no Havok code or headers were used.
Public descriptions of MOPP bytecode exist (NifTools' nif.xml,
`bhkMoppBvTreeShape`); nothing here depends on them: every semantic below
is read from the engine. No disc bytes or dumps are committed;
tests read the disc at run time and are `#[ignore]`.

## Where it lives

| What | Meaning | Evidence |
|---|---|---|
| `sbsp+0x1e0` | Byte block (element size 1): count, pointer. The pointer is the code object below. The count is the object's size (`+0x20`) rounded up to 16. | V (count); E pointer `0x001c3b3e` |
| Engine path | BSP activation `0x001c2910` → `0x001c3b70` → `0x001c3b20` reads `[sbsp+0x1e4]` and passes it to the `hkMoppBvTreeShape` constructor `0x002df760`, which stores it at shape `+0x10` and calls `0x003009b0` (`inc [code+0x24]`). The count is never read. | E |
| `sbsp+0x204` | A second program, optional (breakable surfaces, Guess). Not covered here. | E `jle` at `0x001c293a` |

### The code object

| Offset | Type | Meaning | Evidence |
|---|---|---|---|
| `+0x00` | f32×3 | Origin of the integer space (world units) | E `0x003008e4`, `0x00301762` |
| `+0x0c` | f32 | Scale: integer units per world unit | E `0x003008ea`, `0x00301753` |
| `+0x10` | u32 | `0xff` on every retail BSP; the interpreters never read it | V; E (no reads) |
| `+0x14..+0x1f` | | Zero on retail | V |
| `+0x20` | u32 | Size of the object **including this 0x30-byte header**: bytecode runs from `+0x30` to `+size` | V: on every retail program the last 0x30 bytes before `+0x30 + size` are never reached by any branch, and nothing past `+size` is |
| `+0x24` | u32 | Reference count: 1 on disc, incremented when the shape is built | V; E `0x003009b0` |
| `+0x28..+0x2f` | | Zero on retail | V |
| `+0x30` | u8[] | Bytecode | E `0x003008e7` |

The writer emits exactly this: header, bytecode, zero padding to 16
bytes, and the block count is the padded length.

## Integer space and queries

Coordinates are integers in a 24-bit space per axis: `full =
(p - origin) * scale`. A program walks *frames*: frame coordinate `=
(full >> shift) - frame_offset`. The root frame has shift 16 and offset 0,
so it sees the top 8 bits; rescale opcodes zoom in. Every operand of the
axis opcodes is a byte in the current frame.

The XBE has four interpreters. One byte table per interpreter maps the
opcode to a handler, and the tables put every opcode in the same handler
class (E: tables at `0x00300860`, `0x003016a8`, `0x0030284c`, `0x002fff20`;
dword tables at `0x003007e0`, `0x00301628`, `0x003027cc`, `0x002ffea0`):

| Interpreter | Query | Entry | Evidence |
|---|---|---|---|
| `0x00300290` | Sphere, integer. Setup `0x003008d0`: centre `round(p * scale) - 1` per axis (x87 `fistp` of an f32 product), radius `round(r * scale)`; root state: centre `>> 16`, radius `(r >> 16) + 1`, shift 16 | E |
| `0x003009e0` | Ray between two points, float (setup `0x00301720`; scale times a constant from `0x0045dcac` for the root frame) | E (kind of query: Guess from the setup) |
| `0x00301a60` | Segment or cast with a per-axis tolerance, float | E (kind of query: Guess from its operands) |
| `0x002ff8b0` | Box, integer. Setup `0x00300150`: `lo = round(min * scale) - 1`, `hi = round(max * scale) - 1` per axis; per frame `L = (lo >> s) - offset`, `H = (hi >> s) + 1 - offset`. Its tests differ from the sphere VM's (see [The box query](#the-box-query)) | E |

The interpreter state is 0x28 bytes (E `0x00300290`): the query in frame
units (`+0x00..+0x0c`), frame offsets (`+0x10..+0x18`), key offset
(`+0x1c`), shift (`+0x20`), property 0 (`+0x24`). A branch that recurses
copies the state first, so a subtree's rescales and key offsets never
leak to its sibling.

## Opcodes

`q` is the query's interval on the axis in the current frame (`[lo,
hi]`; the sphere VM uses `[c - r, c + r]`). Multi-byte operands are
big-endian. "Ends" means the current branch returns. Every row is **E**
from `0x00300290` (handler addresses in parentheses); the last column says
whether retail programs use it (**V**).

| Opcode | Bytes | Semantics | Retail |
|---|---|---|---|
| `00` | 1 | Ends (`0x00300746`) | no |
| `01..04` k | 4: `k a b c` | Rescale: shift −= k; offset_i = (offset_i + operand_i) << k; the query is re-derived from full precision at the new shift (`0x00300570`) | yes |
| `05` | 2 | Jump forward by the 8-bit operand (from the next instruction) (`0x0030052e`) | yes |
| `06` | 3 | Jump, 16-bit (`0x0030053b`) | yes |
| `07` | 4 | Jump, 24-bit (`0x00300551`) | yes |
| `09` | 2 | Key offset += 8-bit (`0x003005fb`) | yes |
| `0a` | 3 | Key offset += 16-bit (`0x00300622`) | yes |
| `0b` | 5 | Key offset = 32-bit (`0x00300652`) | yes |
| `10..12` axis | 4: `L H j` | Two-plane split on x/y/z. If `hi < H`: low child only. Else if `lo > L`: high child only. Else both. Low child is the next instruction, high child is `j` bytes after it. Low-child primitives have max ≤ L, high-child ones min ≥ H; L and H may overlap or leave a gap (`0x003003be`, `0x003003cd`) | yes |
| `13..1c` | 4: `L H j` | The same on a diagonal: `13` y+z, `14` y−z+255, `15` x+z, `16` x−z+255, `17` x+y, `18` x−y+255 (operands ×2); `19` x+y+z, `1a` x+y−z+255, `1b` x−y+z+255, `1c` x−y−z+510 (operands ×3). The sphere VM widens its radius to `r + (r >> 1) + 1` (×2 forms) or `4r` (×3) (`0x003002bf..0x003003bc`) | yes (`1b` not seen) |
| `20..22` axis | 3: `s j` | One-plane split: if `hi < s` low only; else if `lo > s + 1` high only; else both. Low may reach `s + 1`, high starts at `s` (`0x00300401`) | yes |
| `23..25` axis | 7: `L H J1 J2` | As `10..12` with 16-bit jumps: low child at `next + J1`, high at `next + J2` (`0x0030043e`) | yes |
| `26..28` axis | 3: `l h` | Bound check: ends unless `q` overlaps `[l, h]` (`0x003004ac`) | yes |
| `29..2b` axis | 7: `l24 h24` | Bound check in full precision (24-bit operands, query unshifted) (`0x003004db`) | yes |
| `30..4f` | 1 | Emit key `op − 0x30 + key offset`; ends (`0x0030079e`) | yes |
| `50` / `51` / `52` / `53` | 2 / 3 / 4 / 5 | Emit 8 / 16 / 24 / 32-bit key + key offset; ends | yes (`52` not seen) |
| `60` / `64` / `68` | 2 / 3 / 5 | Property 0 = 8 / 16 / 32-bit value; nothing in the structure path reads it | no |
| `61..63`, `65..67`, `69..6b` | | Property slots 1..3: they index past the 0x28-byte state onto the return address in this build. Never valid | no |
| `08`, `0c..0f`, `1d..1f`, `2c..2f`, `54..5f`, `> 6b` | | Assert (`0x00300710`) | no |

The two float interpreters do the same tests on float intervals (E, e.g.
`0x00301d74`: `20..22` in `0x00301a60` tests against `[s − tol, s + 1 +
tol]`).

### Keys

A key's type is `key >> 29`; `0x001ef3e0` dispatches on it (E). Type 1
is a structure collision surface, index `key & 0xffff` (E `0x001ef451`:
the 8-byte surface record of the collision BSP's `+0x28` block). Retail
programs also emit types 2, 3 and 4 (V; type 2 carries a 13-bit field at
`key >> 16`, plausibly instanced geometry, Guess). Every structure surface
is keyed exactly once on every retail BSP (V).

### Diagonals and frame floors (V)

Retail programs place a surface past a diagonal plane by one unit more
than per-axis floored frame coordinates allow: the compiler projected the
finer coordinates, then floored. A box model of a diagonal test must
therefore treat each frame coordinate `v` as the finer interval `[v, v +
1)`. With that, the model returns every surface on every sampled query
(below); without it, it missed 970 of 2.3 million. The sphere VM's `+ 1`
on the diagonal radius is the engine's allowance for the same thing.

### The box query

`0x002ff8b0` is the query that keeps a body's contacts with the
structure (E): the BV-tree agent's collide (`0x002ea530` → `0x002e9d90`)
takes the other body's box (`0x002e9140`: its shape's AABB at the
collision tolerance, grown by its linear motion), queries the MOPP
shape's slot `+0x30` (`0x002df640`, or the structure shape's override
`0x001eea50`, which calls it and filters instanced keys), and keeps one
child agent per key returned. A key missing from one tick's answer drops
that surface's contact points (`0x001d06c0`); a biped's ground contact
list (physics component `+0x88`) is filled by the contact callback
`0x00277b80` through `0x001d0400`. The float cast `0x00301a60` is the
static linear cast (`0x002eb670`, registered at `0x002ebc0b`), not this
path.

Its tests, with `L` and `H` as in the table (`H` is one past the box's top
cell):

| Opcode | Box VM (`0x002ff8b0`) |
|---|---|
| `10..1c`, `23..25` (`L_op H_op`) | low child only when `L < L_op`; high child only when `H > H_op`; a box in neither visits neither (`0x002ffa52`, `0x002ffa85`, `0x002ffafe`). Diagonals sum `L` for + terms and `−H` for − terms (lower), `H` and `−L` (upper), against `m·op − k` |
| `20..22` (`s`) | low child when `L <= s`, high child when `H > s` (`0x002ffac7`) |
| `26..28` (`l h`) | ends when `H < l` or `L >= h` (`0x002ffb73`) |
| `29..2b` | ends when `hi < l24` or `lo > h24` (`0x002ffb9d`) |

So `L_op` and a bound check's `h` are exclusive upper bounds here: a box
whose low corner lies in the same frame cell as a primitive's top cell is
cut off from it, where the sphere VM keeps it.

## The model (`mopp::vm`, `mopp::boxvm`)

- `query_sphere(code, centre, radius)`: `0x00300290` to the bit,
  including the centre's `- 1` and the radius's `+ 1`.
- `query_box(code, aabb)`: `0x00300150` and `0x002ff8b0` to the bit,
  the engine's box query above.
- `query_aabb(code, aabb)`: the sphere VM's opcode semantics on per-axis
  intervals. Each box axis becomes `[floor(min) − 1, ceil(max) + 1]` in
  full units; diagonals as above. This is not an engine function; it is
  the generalisation the compiler and the tests need.
- `check::validate(bytecode)`: walks every branch and rejects what the
  engine would assert on or overrun: opcodes outside the table, property
  slots 1..3, reads past the end, rescale below shift 0. Reports the
  opcode histogram, every key a terminal can emit, unreached bytes and
  depth.
- `bounds::key_bounds(code)`: for each key, the box outside which no query
  reaches its terminal (the axis-aligned tests on its path; diagonals are
  ignored, which only enlarges it). Used to carry retail's non-structure
  keys into a rebuilt program.

## The compiler (`mopp::build`)

`build(&[Surface]) -> Code` over polygons with keys:

1. Origin = surface bounds' minimum minus a pad (1 cm + 0.01 % of the
   extent); scale = `(2^24 − 65536 − 4096) / (extent + 2 pad)`, so no
   surface reaches root cell 255.
2. Each surface's box is quantized to full units: `floor − 2`, `ceil + 2`
   (covers the sphere VM's rounding).
3. Recursive split: zoom (`01..04`) as far as the subtree's union fits a
   finer frame (subtrees of 8 or more surfaces), narrow the region with
   bound checks `26..28` where that cuts at least 1/8 of it (1/32 at a
   leaf), move the key offset (`09`/`0a`) where terminals get shorter, then
   split at the median centre on the axis of widest spread with `10..12`
   (low child ≤ 255 bytes), `23..25` (≤ 64 KiB) or `23..25` plus a `07`
   jump to the high child. Leaves emit `30..53`. The root sets the key
   base with `0b`.
4. Upper bounds are exclusive: a split's `L` and a bound check's `h` are
   one past the top cell of the surfaces below them, as the box query
   needs (see [The box query](#the-box-query)); the sphere query only
   gets one cell looser. Through converter version 71 they were the top
   cell itself, and the box query dropped a surface once a body's box
   started in that cell. That is the suspect for converted Blood Gulch's
   blue ramp, where the live player's ground contact went at x 43.56 and
   the player fell; `h1convert/tests/ramp_contacts.rs` runs the box query
   there.
5. Invariant: every surface of a node lies in `0..=254` of the node's
   frame (a zoom stops where the union would pass 254), so every operand,
   exclusive tops included, fits a byte; the tracked region follows the
   sphere query's branch rule (a query in a gap between L and H still
   enters the low child).

It emits only `01..04`, `07`, `09`, `0a`, `0b`, `10..12`, `23..28`,
`30..53` (no diagonals, no `29..2b`, no properties). Byte equality with
retail is not a goal.

## Proof on the disc

`cargo test -p mopp --release --test disc -- --ignored --nocapture`
(`H2_DISC` or `disc/halo2.iso`). Per BSP, 400 boxes around random surface
vertices, half-sizes 2 cm to 8 m plus one in 16 up to 22 m; truth is every
structure surface whose polygon touches the box shrunk by 1 mm (exact
separating-axis test).

| Program | Validates | Box misses | Sphere misses | Returned / truly touching | Returned / box-overlapping | Bytes |
|---|---|---|---|---|---|---|
| Retail, 78 BSPs | yes | 0 of 2,321,215 | 0 | 1.05 | 1.03 | 10.37 MB |
| Ours, 78 BSPs | yes | 0 | 0 | 1.04 | 1.02 | 10.15 MB |

(Sphere: the engine-exact sphere VM with the sphere around each box.
Box misses are the box model's, `query_aabb`. The engine's box query,
`query_box`, also misses 0 on both. Ours is version 72's compiler, with
exclusive tops: 10,148,336 bytes. While the low child's tracked region was widened by a cell,
it was 11,047,968 bytes.)
Ours keys every structure surface exactly once and leaves no byte
unreached. Per BSP with more than 100 surfaces, ours returns 1.01 to 1.28
surfaces per touching one (retail 1.02 to 1.39); on the eight BSPs of 6 to
88 surfaces both are looser (ours 1.67 to 3.3, retail 1.24 to 2.95), over
few queries. Size per BSP is 0.16 to 1.14 of retail's (retail also keys
its non-structure surfaces).

### Replacing a retail level's program (`map::build`)

`tests/disc.rs` `replaced_retail_levels`: for lockout, colossus,
cyclotron, headlong, coagulation and zanzibar, the program built from the
level's structure surfaces plus retail's other keys (as
`bounds::key_bounds` boxes) is written over retail's block in the BSP
region (it fits), the count updated, and the map rebuilt with
`TagSet::from_map` and `build`. The result reads back with a valid
checksum and our block, returns every touching structure surface on the
sampled boxes, and returns every non-structure key retail's program
returns on them.

To try one in the game (the user's run; agents don't run the game):

```sh
H2_DISC=disc/halo2.iso cargo run --release -p mopp --example replace -- lockout ~/h2-mopp
H2_MAP_DIR=~/h2-mopp h2 play disc/halo2.iso --level lockout
```

Stderr should say `map override: maps\lockout.map from ...`. Then, in the
level: throw grenades onto floors, walls and ramps (they should bounce and
rest, not sink); drop a weapon (it should land and lie on the floor);
on a map with vehicles (headlong, coagulation, zanzibar) drive and flip
one against walls and slopes. Also check that crates and other physics
props settle as on the retail map. Remove the file or unset `H2_MAP_DIR`
to go back.

## API for the converter

```rust
/// The sbsp+0x1e0 block's bytes: element size 1, count = returned length.
mopp::build_structure(bsp: &tags::collision::Bsp) -> Vec<u8>
```

Keys every collision surface as structure key `0x20000000 | index`, so a
BSP over 65,536 surfaces cannot be keyed (the engine masks 16 bits;
`h1convert::structure_mopp` refuses one). Write
the bytes as the block's elements and the length as its count; nothing
else in the `sbsp` refers to the program. `mopp::build` takes any keyed
polygons, for callers that need more.

## Open questions

- `+0x10` (always `0xff`): Havok's meaning is not established; nothing
  in the three interpreters reads it.
- Key types 2, 3, 4 (beyond the `0x001ef3e0` filter) are not decoded
  here; a converted level only needs type 1.
- The second program at `sbsp+0x204`.
- Whether the engine's x+y+z sphere radius (`4r`) can fall short of the
  one-unit diagonal floor allowance for radius-0 queries is untested; our
  compiler emits no diagonals.
- Only a game run shows Havok bodies colliding with a rebuilt program
  (steps above).
