# HUD shader selection and constant upload

Evidence for [the native HUD overlay](../native-post-hud.md).

## 2026-11-24 - from the widget to the bound combiner

Read at `12ae05214` with `h2 disasm` on the owner's retail executable.
To reproduce, disassemble the following entries and follow the calls below.

```sh
h2 disasm 0x1bbf0 110
h2 disasm 0x16b90 295
h2 disasm 0x18ee0 480
h2 disasm 0x1b050 28
h2 disasm 0x22aa16 195
```

- **Selecting the shader.** At `0x1bcb9..0x1bcdd`, EDI is the shader
  datum. The code checks its group and reads its tag data through the
  runtime index. At `0x1bd39..0x1bd47` it passes the datum and the
  render context at `0x485b48` to `0x16b90`. When a new definition is
  needed, `0x16e6a` tests the implementation's flags. One branch copies
  the pixel shader definition from implementation offset `+6` into
  context offset `+0x1424`. The other calls the fragment assembler
  `0x1ba00`. A template name alone does not select the resulting combiner.
- **Filling constants.** Calls at `0x16ea9`, `0x16ebf` and `0x16eea`
  apply the instance values, animations and engine pixel bindings.
  `0x19e0b` packs an engine value, then the masked write at `0x19e39`
  preserves channels owned by other bindings. See the [animation binding
  trace](2026-11-24-shader-constant-animation.md) for animated colours.
- **Binding and reusing it.** At `0x16f37`, context byte `+0x1514`
  selects a new shader upload through `0x1b050` or a constants-only
  update through `0x18ee0`. The former passes the definition to
  `0x3fa3d0` and clears the byte. The latter walks two dirty masks
  (`+0x1518`, `+0x151c`), reading each stage's packed C0 or C1
  at `+0x144c` or `+0x146c`, respectively. Bit eight in each mask
  selects the final combiner constant at `+0x14d0` or `+0x14d4`;
  `0x194df..0x19515` emits these separately. Both paths use the same
  populated definition. Caching just the program cannot freeze its constants.
- **Meter input.** `0x22b181..0x22b197` copies four evaluated widget
  inputs to `0x4e6980`. Before the draw at `0x22b397`, `0x22b380`
  can overwrite the first input. Shield layers write the first two inputs
  at `0x22b4dd` and `0x22b4e5`, select colours and draw at
  `0x22b55c`. Reading these globals at the bitmap loop's first gate
  precedes the widget's writes.
- **Text colour.** `0x22aacb..0x22aade` selects the text widget's shader
  datum at `+0x24` and commits texture bindings. Its glyph draw at
  `0x22aca8` goes through `0x1fb10`, which passes one as the last
  argument to `0x1f070`. At `0x1f253` this becomes byte `+0x97`
  of the screen quad record. The check at `0x3619d` skips installing
  the default combiner. The inherited shader and the sampled glyph
  determine the pixel colour.

### Which call draws a bitmap widget

`h2 disasm 0x22b2d0 120` shows the special-type switch. The ordinary
type (zero) jumps to `0x22b5cb`. With widget flag `0x10` clear and
the draw enabled, it reaches the quad call at `0x22b690`, returning to
`0x22b695`. The call returning to `0x22b39c` is inside the type-seven
loop, whose count is the HUD state's word at `+0x22c`. Each iteration
loads its own colour and first input. Type five loops over a single call
at `0x22b55c`, advancing its layer counter at `0x22b565` up to five
layers. These three return addresses identify different draw paths.

### Materials referenced by retail widgets

At the same revision, the campaign and multiplayer maps' local `nhdt`
bitmap and text blocks reference eight distinct template names. To repeat
the census, use `h2 tags MAP nhdt` for the roots, walk the bitmap and
text blocks using `hud_edit::layout`, and resolve each distinct shader
name with `h2 shader MAP NAME`. Zanzibar contains all eight templates.
These are references in tag data, independent of a widget's visibility.

The legacy meter, custom meter, simple flash, gradient flash and simple
custom templates sample the widget bitmap on stage zero and use source
alpha and inverse source alpha blending. The shield template samples the
same engine texture but adds the shader result to the destination scaled
by source alpha. Text flash samples the glyph page instead. All these
passes write RGBA. The waypoint emblem is the exception among the bitmap
widgets: two Zanzibar widgets reference it, its stages sample separate
background and foreground bitmap tags, and it writes RGB only.
