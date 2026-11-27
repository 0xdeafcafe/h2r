# Shaders: what the retail corpus holds

Evidence for [shaders.md](../shaders.md) and [shaders-pixel.md](../shaders-pixel.md). Nothing here states what the code does today; the design pages do. Entries are newest first.

## 2026-11-27: corpus counts and the PSDEF offset check

Commit: `f7f5604cd`. Method: `h2 extract-shaders` over the owner's disc (cache build `02.09.27.09809`, all 30 maps), then `shaders` corpus tooling. Reproduce:

```sh
cargo run --release -p cli -- extract-shaders --out $TMPDIR/h2-shaders     # disc: $H2_DISC or disc/*.iso, or --disc
cargo run -p shaders --example corpus -- $TMPDIR/h2-shaders                 # stats + validation
cargo run -p shaders --example corpus -- $TMPDIR/h2-shaders --dump <file>   # disassembly + WGSL
H2_SHADER_CORPUS=$TMPDIR/h2-shaders cargo test -p shaders --test corpus     # also GPU vs interpreter
H2_SHADER_CORPUS=$TMPDIR/h2-shaders cargo test -p d3d --test shader_corpus -- --ignored
```

| What | Count |
|---|---|
| distinct vertex programs / pixel shader definitions | 353 / 149 (all decode, translate, pass naga, no warnings) |
| `vrtx` tags with the root `u16` equal to 1 | 2,261 |
| `vrtx` program entries: empty / holding `'x' 0x20` | 765 / 11,072 (11,837 in all) |
| entries with a non-empty `u16` block: block equals the `v` registers read / differs | 10,712: 1,985 / 8,727 |
| `spas` tags / implementations | 2,688 / 4,593 |
| definitions with `MUX_MSB \| UNIQUE_C0 \| UNIQUE_C1` / with no general combiners | 140 / 9 |
| definitions with embedded `c0` values | 38 |
| implementations with general combiners / texture stages only | 3,358 / 1,235 |
| distinct pixel shaders over the resolved materials (`h2 shader --all all`) | 2,931 of 307,944 implementations, all translate without a warning |
| materials with `COLORSIGN` set | 30 implementations, all stage 0, always `0xE0000000` |
| resolved stage states with `COLORKEYOP` set | 0 of 809,233 |
| `BLENDOP` | always `ADD` |
| `SRCBLEND` | `CONSTANTCOLOR` in 50,046 implementations, `DESTALPHA` in 41,525 |
| `DESTBLEND` | `CONSTANTCOLOR`/`INVCONSTANTCOLOR` 10,359 times |
| `BLENDCOLOR` | mostly `0x00FFFFFF`: RGB written, alpha left to the constant's zero |

Where a PSDEF starts inside a `spas` implementation. Each even offset 0 to 0x42 is parsed and scored against the encoding's invariants (combiner count at most 8 with only the documented flag bits, texture modes at most 0x12, dot mappings at most 7, input selectors inside their fields, compare modes in 16 bits, active stage outputs inside 20 bits, not all zero), then translated. The extractor refuses to write a corpus unless `+6`, and only `+6`, passes for every implementation.

| offset | pass invariants | translate cleanly | with combiner stages |
|---|---|---|---|
| **0x6** | **4,593** | **4,593** | **3,358** (all that have any) |
| 0xE | 4,467 | 4,467 | 60 |
| 0x12 | 4,274 | 4,274 | 0 |
| 0xC | 4,003 | 4,003 | 757 |
| 0xA | 1,095 | 1,095 | 941 |
| others | at most 2,905 | | at most 484 |

The last column carries the evidence: a misaligned window often reads a combiner count of 0, which no invariant contradicts. Only at `+6` do the 3,358 implementations with general combiners decode as combiner programs.

Provenance of the run: `default.xbe` SHA-256 `03215919bb7163259257d361f4c7bf802a7ab12aa85e2689436369b5c427935d`, manifest SHA-256 `961edf58b37cd0e7204f88dadc56120433d5a7ddf052b9e48a0bfc69336a4d91`.

Tally of register-naming slots in the 149 definitions (RGB portion / alpha portion):

| What | Count |
|---|---|
| input mappings: unsigned identity | 1,032 / 503 |
| invert | 99 / 47 |
| expand | 71 / 51 |
| expand-negate | 5 / 3 |
| half-bias | 3 / 0 |
| half-bias-negate | 12 / 19 |
| signed identity | 10 / 35 |
| signed negate | 2 / 8 |
| output mappings: identity | 428 / 458 |
| x2 | 55 / 10 |
| x4 | 1 / 16 |
| /2 | 2 / 2 |
| −½ bias | 0 |
| AB dot / CD dot | 64 / 37 |
| mux | 8 (RGB) / 49 (alpha) |
| blue-to-alpha | 0 |
| final combiner `E·F` reads | 36 (9 as A, 24 as B, 3 as D) |
| final combiner `G` | `r0.a` 47, `t0.a` 24, blue 10 |
| definitions reading the fog register | 5, all in the final combiner |
| vertex programs writing `oFog` | 64 of 353, one write each |

The 64 `oFog` writes by shape: 54 are a dot with a constant (`dph oFog.xw, rN, c[35]`, or `c[26]` in 6), 9 multiply a `.w` (`mad oFog.w, rN.w, c.x, c.y`) and 1 moves a vertex attribute (`mov oFog.xw, v7.w`). A backward slice over the temporaries finds no `rsq` behind any of them. `corpus_fog_census` in `tests/corpus.rs` prints the tally.

Texture-mode use: no definition uses `dot_zw`, `brdf` or `dot_rflct_spec_const`, and no push of `SET_EYE_VECTOR` has been seen. All 10 dot-product stages in the 149 definitions, and every dot stage across the 307,944 implementations, use `minus1_to_1_d3d`. No HILO mapping is used. 30 implementations have bump-env stages, each reading stage 0 from stages 1, 2 and 3.

Register reads: no definition reads a texture register of a `NONE` stage before writing it; one reads `r0.rgb` before writing it (zero on both sides) and one reads the initial `r0.a`, with stage 0 textured.

Fog constants: D3D's exp scales (`0x45E378`, `0x45E374`) differ from the closed forms by about 4e-6 relative. The ignored `crates/platform/d3d/tests/fog_xbe.rs` reads them from `$H2_XBE` and checks that tolerance.

Float behaviour on the host: in the random vertex test, 352 of 2,472 non-finite expected values differ between interpreter and GPU, all in two programs (Metal fast math). The random vertex test compares finite outputs at a relative tolerance (over 1,000 exceed 1e4), and the 248,512 retail-corpus comparisons (no magnitude cut-off) all match. The w-buffer fix for equal-depth passes: 88% of pixels failed `w_buffered_passes_with_ulp_different_w_store_equal_depth` without it, under 1% with it.
