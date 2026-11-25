# Halo 1 multiplayer conversion: measurements

Evidence for [../multiplayer.md](../multiplayer.md).

## 2026-11-19: teleporter receiver brightness against Halo 1

Commit `ea154e2cc`. Method: the ignored disc test
`teleporter_receiver_brightness_against_halo1` in
`crates/tools/h1convert/tests/shad_rebuild.rs` computes, from the disc's
bitmaps and the shader constants, the face-on brightness per channel (red,
green, blue) of Halo 1's receiver shaders and of the conversion's. Run it
with `H1_ISO` and `H2_DISC` set and `--ignored`.

The cone (Halo 1 `dust x dust x ramp`, no doubling):

| | Mean | Peak |
|---|---|---|
| Halo 1 | (0.012, 0.030, 0.014) | (0.148, 0.362, 0.175) |
| Conversion | equal by construction (the dropped dust is folded in as its mean) | (0.062, 0.154, 0.073) |

The folded dust's peak is averaged away. Drawn as its least tiled map alone
(the grey dust), the cone's peak was 0.61 in every channel.

The shield, lit, over the picture (simulated before DXT5; the test decodes
the mask the converter writes):

| | Peak | Mean |
|---|---|---|
| Halo 1 | (0.063, 1.0, 0.134) | (0.005, 0.269, 0.047) |
| Conversion | (0.020, 1.0, 0.173) | (0.005, 0.269, 0.046) |

The shield's glow on a dark line:

| | Peak | Mean |
|---|---|---|
| Halo 1 | (0, 0.262, 0.040) | (0, 0.067, 0.014) |
| Conversion | (0, 0.247, 0.051) | (0, 0.066, 0.014) |

Rebuilds that lost: the first plasma rebuild drew the picture's mean colour
masked by its alpha (0.10 to 0.13, nearly flat), peak (0.0005, 0.027,
0.004), 37 times too dim, shapeless and with no glow. Before that the
plasma drew `energy plasma`, white, peak 1.0 in every channel.
