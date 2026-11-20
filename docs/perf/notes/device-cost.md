# D3D device CPU cost: measurements

Evidence for the benchmarks described in [../profiling.md](../profiling.md), "Device CPU cost (crates/d3d)".

## 2026-11-15: per-draw cost of the device on the push-buffer frame

Commits: `9c6602428` (the benchmark and the first cuts), `2859f4328`
(texture state read as one access). Machine: shared Apple Silicon laptop.
Method: `cargo bench -p d3d --bench pushbuf`, synthetic frame of 1,200
draws against a null backend; compare p10 or median, not the mean. To
reproduce, check out the commits and run the command.

Result: the device's own per-draw work went from a p10 of 5.6 ms a frame to
3.0 ms a frame on that benchmark. It does not measure whole-game CPU per
frame or other upload policies.

Per-draw costs removed in those commits: the linear slot search in
`SimpleEncode::mirror_slot` (now a table), the per-draw guest-slot to
state-type lookups in `StateWords::translate` (`state::Translator`, plus a
memo of the last translation while the arrays are unchanged), the
pixel-shader decode in `sampled_stages`, heap reads of fixed-size headers
(`guest::read_array`, state arrays read straight into arrays), per-word
constant-shadow writes, and the three passes over the index list in
`draw_indexed_vertices`.

## 2026-11-18: vertex stream pool

`cargo bench -p d3d --bench streams` was added with the pool changes: a
stream is appended to an empty pooled buffer straight from guest memory
(`GuestMemory::read_append`, no zero fill), the pool hands out the smallest
buffer that fits, and immediate draws fill pooled buffers too. Before the
last of these, immediate draws' fresh buffers overflowed the pool, which
then freed the large buffers the next indexed draws had to allocate and
fault in again. No numbers were recorded for this entry.
