# Shader animation writes at draw time

Evidence for [shader instances, templates and passes](../../formats/shader-instances.md).

## 2026-11-24 - constant destinations and sources

Read at `0aee0afea` using the owner's retail executable and `h2 disasm`.
To reproduce the instruction traces:

```sh
h2 disasm 0x000176a0 192
h2 disasm 0x00017960 146
h2 disasm 0x0001b580 134
h2 disasm 0x0001b790 129
h2 disasm 0x00017550 94
h2 disasm 0x0001b230 54
h2 disasm 0x0013bc00 170
h2 disasm 0x00131bb0 34
h2 disasm 0x00131e50 36
```

Both loops walk four-byte bindings. The pixel loop at `0x176fa..0x1771c`
and the vertex loop at `0x179d3..0x179f1` use byte 3 to index the range,
mask its start to nine bits, and sign-extend the first reference's property
index. Neither loads the reference's component. By comparison the bitmap
loop at `0x175aa..0x175bd` reads the count, advances each reference at
`0x17652`, and reads its signed component at `0x17635`.

### Pixel combiner constants

`0x176e3..0x17738` splits byte 2 into the output mode (low three bits)
and scalar transform (next three bits). It clears a four-float temporary
and calls `0x1b580`. For modes 0–3 that helper evaluates the property,
maps it through the header range when the high flag nibble is clear, and
writes the transformed scalar at the mode's float index. The branches at
`0x1b607..0x1b6fe` implement the complement and the four sine/cosine
forms shared with engine scalar constants. Mode 4 branches at `0x1b5a7`
to the colour lookup, then expands its packed RGB bytes to floats.
Modes 5–7 place the scalar beyond the four channels read by the caller.

`0x1773d..0x177c5` clamps the four channels and reorders them to ARGB.
`0x131e7d..0x131eab` scales each by 255 and converts with x87 integer
rounding. This path applies no signed-to-unsigned half-scale bias.

`0x177ca` loads bytes 0 and 1 as one little-endian word. With fragments,
`0x177e7..0x1781e` compares the selected fragment's integer input with
bits 3–5 and skips a mismatch. Bits 0–2 pick the fragment. Bits 6–9
select the register. Register 8 branches to the final combiner at
`0x17843`. Other registers add the fragment's first stage at `0x17860`.
Bit 10 selects C1. The mask lookup at `0x1783c` is shared with static
colour bindings. The read/modify/writes at `0x17879`, `0x178a3`,
`0x178cf` and `0x178f5` preserve the other channels and set the bank's
dirty bit. A later binding of the same channel wins.

### Vertex constants and attribute defaults

`0x179f4..0x17a0b` takes the register from the low nibble of the word,
selects the bank with bit 4 and multiplies the index by the float4 stride.
The upload functions at `0x18d70` and `0x18e80` use the default attribute
and hardware constant banks respectively. The latter adds 18 to the index.
Bits 5–9 select the write. At `0x17a4e..0x17a8c`, modes 0–3 apply the
scalar output range and store one float. Mode 4 calls the colour lookup at
`0x17aa6` and writes three floats at `0x17ade..0x17af3`, preserving W.
The vertex loop sets the register dirty after each binding at `0x17b12`.
It does not read byte 2.

For RGB, `0x13bc00` takes the evaluator's raw result, before the scalar
range mapping. Its constant-function shortcut returns the packed word at
header +4. Otherwise the high flag nibble selects grey, a single packed
colour or two to four stops. The two-stop branch at `0x13bca5` uses +4
and +16. The three-stop branch at `0x13bd10` also uses +8, and the
four-stop branch includes +12. Interpolation operates on the packed
channels with a signed difference and a 14-bit fractional weight.
`0x131bb0` extracts R, G and B and scales each byte by 1/255.

### Vertex texture transforms

For modes 5–20, `0x17afa..0x17b0d` passes bits 10–12 and the mode to
`0x1b790`. It bypasses the property evaluator. The source is the texture
slot's transform: `0x17632..0x17642` writes its fields from bitmap
properties before the stages bind. Components 1–3 are scales, 4–6 are
offsets, and 7 is rotation in turns. The static bitmap bind at
`0x16fb4..0x16fdd` sets the frame and LOD and leaves the transform in
place. A slot field can therefore depend on an earlier draw.

With scales `sx,sy,sz`, offsets `dx,dy,dz`, and sine/cosine `s,c`
of the rotation, `0x1b821..0x1b879` computes the diagonal
`rx=sx*c`, `ry=sy*c` and the translations
`tx=dx+(1-c+s)*sx/2`, `ty=dy+(1-c-s)*sy/2`. The branch at `0x1b7e2`
uses zero sine and unit cosine for a near-zero rotation. The stores reached
from `0x1b883` give the following writes. Every omitted lane survives.

| Mode | Destination lanes | Values |
|---|---|---|
| 5–8 | x, y, z or w | rx |
| 9, 10 | xy or zw | rx, ry |
| 11, 12 | xy or zw | tx, ty |
| 13 | xyzw | rx, ry, tx, ty |
| 14, 18 | xyzw | rx, −sx·s, 0, tx |
| 15, 19 | xyzw | sy·s, ry, 0, ty |
| 16 | xyz | rx, ry, sz |
| 17 | xyz | tx, ty, dz |
| 20 | xyzw | 0, 0, sz, dz |
