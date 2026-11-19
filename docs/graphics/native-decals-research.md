# Native decal investigation

This is an incomplete static census, not a basis for suppressing any guest
rendering. Addresses and offsets are research points from the guest code;
no draw has yet been compared at runtime.

## Confirmed paths

- The sorted transparent list calls the handler in each entry at `+0x20`
  with `entry+0x24`, in the order produced by its sort (`0x0001e370`).
  `0x00042760` adds entries for records whose `+0x1c` is 3, with handler
  `0x000423c0`, payload `(record index, caller argument)` at `+0x24`, and
  sort class 3. This alone does **not** establish that these are decals.
  `0x000420a0` adds class-3 entries with handler `0x00041cc0` for records
  whose `+0x1c` is 1. Neither handler should be classified as a decal
  without tracing the record producers and their draw bodies.
- The runtime decal array is initialized by `0x0017d220` (the name was
  previously identified at `0x0017d23c`, see `engine-map.md`). It calls
  `0x00043890`, which initializes the associated geometry pool. The decal
  geometry construction at `0x00180f87` calls `0x000439d0` to obtain a
  dynamic buffer address. `0x00044040` walks the decal array via the heads
  at `[0x004ea94c]`, indexed by the layer set at `0x00043a4f` and its
  first argument. It resolves a datum, selects state and issues geometry,
  following the record's `+0x34` link. Its two-argument calling sites and
  the surrounding render pass still need tracing.
- `0x00043a40` selects among several shader/state setups by its argument,
  and records that argument at `0x004c8790`, which `0x00044040` reads.
  The effective depth test and bias, stencil, blend, target and texture
  state cannot be inferred from this setup in isolation: inherited state
  and the actual draw site must be captured together.

## Remaining evidence before native admission

Trace every caller and handler to distinguish sorted and non-sorted decal
sites, including late and first-person views. Capture each actual draw's
order and effective receiver depth/bias, stencil, blend, shader, textures,
geometry and target beside the candidate native draw. The comparison must
identify the view, site and draw, handle multiple draws per handler, and
report unpaired draws. Only suppress an entire guest handler after all its
resources and state have been resolved and its native work has been accepted
in the same ordered dispatcher as the other transparent draws. Until then,
all of these draws remain on D3D.
