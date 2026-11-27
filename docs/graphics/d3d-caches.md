# D3D texture and vertex caches

Status: Built. The texture cache, the vertex cache and the invalidation contract that keeps both correct.
Code: `crates/platform/d3d/src/vertex_cache.rs`, `crates/platform/d3d/src/device.rs`. Tests: `crates/platform/d3d/tests/vertex_cache.rs`.
Switch: `H2_VERTEX_CACHE_MB`, `H2_BOOT_UPLOAD_ALWAYS` in [switches.md](../runtime/switches.md). Part of [d3d.md](d3d.md).

| Cache | Keyed by | Valid while | Bounds |
|---|---|---|---|
| texture | texture header, plus the palette header for P8 | identity words, palette words and the `WriteTracker` generations of the texel and palette ranges are unchanged | none |
| vertex window | guest address and length of the window | no page it covers has a newer tracker generation | `H2_VERTEX_CACHE_MB` (default 128) |
| index list | guest address and length, in the contiguous `0x80000000 \| pa` view | same | a quarter of the vertex budget, 65536 entries |

All three rest on the [invalidation contract](#invalidation-contract): the runtime reports every guest store the device did not make.

## Textures: lazy upload, dirty tracking

A texture is uploaded at the first draw that samples it, not when it is
bound.

- The cache is keyed by texture header plus, for P8, palette header: one
  header bound with two palettes is two host textures.
- An entry is valid while all of these are unchanged:
  - the identity words (`Data`, `Format`, `Size`);
  - the palette's header, `Common` word (which carries the size bits),
    data address and length;
  - the `WriteTracker` generations of the texel range and the palette
    range.
- `UploadPolicy::Verify` hashes texels and palette entries together.

## Vertex cache

Most of a level's vertices (BSP, object models) never change, but every
draw would copy its whole vertex window out of guest memory, and the
render thread would stage the same bytes in its upload ring. The vertex
cache (`crates/platform/d3d/src/vertex_cache.rs`) keeps them instead, under the
same write-tracker contract as textures.

- **Key.** A vertex window is keyed by guest address and length (the
  stream's `Data` translated, plus offset and first vertex), so two index
  windows over one buffer are two entries. An index list for
  `DrawIndexedVertices` is keyed by its address and length, and only when
  it lies in the view the tracker reports (`AddressMap::to_virtual` of
  its own `to_data`: the contiguous `0x80000000 | pa` view); a list
  elsewhere is read every time. `DrawVerticesUP` and immediate-mode
  vertices are never cached.
- **Validity.** An entry remembers the tracker generation when it read
  the bytes, and is clean while no page it covers has a newer one. The
  tracker is page-granular: a four-byte store re-reads every window on
  that page.
- **Hit.** The draw carries the snapshot (`StreamData::Cached`, or
  `IndexData::Cached`: the smallest and largest index and the list
  rebased by the smallest, so a clean list is not read, scanned or
  rebuilt). No copy on the guest's thread. Each snapshot has an id that
  is never reused; `GfxBackend` passes it to `gfx::Renderer`
  (`Draw::stream_ids`, `Draw::index_id`), which keeps a GPU buffer per id
  (`gfx::persistent`) and binds it again instead of staging the bytes.
- **Admission.** A window is copied the first time it is seen (a *ghost*
  entry remembers it) and snapshotted at its next use if still clean. A
  window found written twice counts as dynamic (particles, decals,
  streamed or skinned vertices) and is snapshotted again only after two
  clean frames, so a buffer the guest rewrites every frame costs a lookup
  and the old copy, not an allocation and a free.
- **Bounds.** Both caches drop least recently used entries past a budget:
  `H2_VERTEX_CACHE_MB` (default 128) of vertex snapshots, a quarter of it
  for index lists, 65536 entries each; the renderer holds at most the
  same budget of GPU buffers.
- **Policies.** `UploadPolicy::Verify` reads the guest bytes on every hit
  and compares them with the snapshot; a difference raises
  `Event::MissedInvalidation` (header 0 for an index list), drops the
  snapshot and draws the bytes just read. `UploadPolicy::Always` bypasses
  both caches. `Device::set_vertex_cache(None)` turns them off for tests
  and benches: every draw copies.
- **Counters.** `Stats::vertex_bytes_copied`, `vertex_bytes_reused`,
  `vertex_cache_hits`, `vertex_cache_fills`, `index_cache_hits`; frame
  stats get `vtx_copied` and `vtx_reused` (bytes per frame).
  `gfx_backend::GfxBackend::persistent_vertex_stats` counts GPU buffer
  hits and uploads.
- **Evidence.** `tests/vertex_cache.rs` runs the same frames with the cache on and off (static, a page rewritten, four bytes rewritten, an index list rewritten, a buffer freed and its memory reused, a buffer rewritten every frame) and requires identical draws through the recording backend and the render thread, and identical pixels on the GPU. `cargo bench -p d3d --bench streams` measures the cost; numbers are in [notes/d3d-caches.md](notes/d3d-caches.md).

What it relies on is rule 1 below: a guest store into vertex or index
memory the barrier does not report leaves the cache drawing old
vertices. Verify finds such a store.

## Invalidation contract

The runtime has to keep these rules:

1. It reports every guest store the device did not make, by guest
   virtual address: exactly through `Device::note_write(addr, len)`, or
   as whole 4 KiB pages through `Device::note_written_pages(addr, len)`
   (the boot's barrier, which also reports the device's own stores back).
   This covers lifted code, the interpreter, and kernel or DMA-like copies.
   Only the exact form tells a lazily held render-to-texture level which
   bytes the guest stored (see [Render-to-texture](d3d-render-targets.md#render-to-texture)).
2. A write must be reported before the next D3D entry point runs.
3. The runtime folds aliases first: physical and contiguous views map to
   the `0x80000000 | pa` view the device uses.
4. Only pages that back a cached resource record a generation. The tracker
   exposes a watched-page bitmap (`WriteTracker::watched_bitmap`) so a
   write barrier can filter cheaply.

Fallbacks:

- `UploadPolicy::Verify` hashes on every use and reports
  `Event::MissedInvalidation` when the barrier missed a write.
- `UploadPolicy::Always` re-uploads every time.
