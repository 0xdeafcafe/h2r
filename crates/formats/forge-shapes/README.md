# forge-shapes

The pieces of Forge's block-out: a definition (`PieceDef`: a shape, a size in
units, a param and materials), its canonical name, and the generators that turn
it into a `Solid`, a closed shell of convex faces and the convex hulls
Havok collides with. The design is
[docs/game/forge-blockout.md](../../../docs/game/forge-blockout.md).

## Where it sits

The `formats` domain, with no dependency on another crate of the workspace, so
the document (`forge-doc`), the tag writer (`content-kit` (its `blockout` module)) and Studio all use
it. It reads no disc and no file.

## Use

- `PieceDef::validate` refuses a size outside 4 to 2,048 units, a param its
  shape does not take and a `Map` material that is not a `shad` key, each by
  name (`Error`).
- `PieceDef::name` is `<shape>_<x>x<y>x<z>_<param>_<digest>`, the digest the
  first 12 hex digits of the SHA-256 of `canonical_json`. `ParsedName::parse`
  reads one back.
- `PieceDef::generate` (or `Shape::generate`) gives the `Solid`: `shell` faces
  wound counter-clockwise from outside, each with a role and origin-aligned
  texture coordinates, and `hulls` whose union is the shell's inside.
- `STARTER` and `starter()` list the sizes the kit installs ahead of a document.

## Notes

- Lengths are `units as f32 / 100.0` and the `+ - * /` of `f32`; a cylinder's
  and an arch's round points come from the rational table in `circle`, not
  from `sin` or `cos`.
- The five natural shapes (`natural`) are each one convex hull of at most 34
  seeded points (`noise`, `hull`): the param is the seed, the bounds are the
  size, the lowest point is on z = 0 and a rock has at most 64 faces. The hull
  is exact integer arithmetic on a 4,096 grid.
- Tests are on invented sizes. The golden hashes of `src/tests/golden.txt` are
  rewritten by running the tests once with `H2_BLESS_GOLDEN=1`.
