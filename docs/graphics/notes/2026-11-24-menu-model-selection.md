# Model widget selection and camera

At base `4d40837d1`, reproduce with `h2 disasm 0x002b0baa 235`,
`h2 disasm 0x0011c0b0 52`, `h2 disasm 0x000bb760 12` and
`h2 disasm 0x000c38e0 45` against the retail Xbox executable.

| Site | Evidence |
| --- | --- |
| `0x002b0bb5` | The selector at `0x0054d598` gates the entire callback. Only selector 4 reaches the definition at widget +0x70. |
| `0x002b0bd1` to `0x002b0c1d` | Definition +0x0c/+0x10 gives a signed count and pointer to 32-byte object names. The loop ignores empty names, looks up each scenario index and keeps the resolved datum in order. Each accepted datum also receives a state update at `0x000bbf40`. |
| `0x0011c0b0` | The scenario pointer at `0x004e0350` leads to name count +0x48 and table +0x4c. The first exact NUL-terminated byte match in 36-byte records returns its index. |
| `0x000bb760` | Indices 0 through 639 select words from the handle array pointed to by `0x004de2d0`. The caller discards the absent datum. |
| `0x002b0c24` to `0x002b0c70` | Definition +0x14/+0x18 gives the separate light-name block. The light resolver walks live light data at `0x000c38e0`. The view requires at least one resolved primary object. |
| `0x002b0c76` to `0x002b0d2b` | The draw argument supplies parent bounds. Definition +0x38 supplies the relative viewport. The draw clamps FOV at +0x34, converts it with the live multiplier and computes split-screen offsets. |
| `0x002b0d30` to `0x002b0db6` | Definition +0x28 supplies camera center, minus the componentwise product of scale (+0x1c) and widget animation (+0x58, axes X/Z/Y). The routine normalizes its two basis vectors. |
| `0x002b0dd5`, `0x002b0e12`, `0x002b0e1b` | Before drawing the view the game installs the resolved object and light lists. After the view it can draw additional widget text. |

The sorted-list reader accepts inactive views and empty object selections
with no light names. Active views retain the nested scene call, including
object state updates, light selection and the optional text tail.

At the nested call the position pointer is EAX, the up-vector pointer is
ECX and the forward-vector pointer is the fifth stack argument
(`0x002b0df3`, `0x002b0e0c`, `0x002b0e0f`). The callee at
`0x0002c560` copies these into successive position, forward and up fields
starting at its ESP +0x38. The material-group admission hook reads this
view and the ordered object handles before it admits indexed geometry.

For a full-screen parent and canvas, the nested call records the model view
and its viewport in the open frame scene. The scene summary includes the
camera and handles. Geometry still uses the nested draw's material and
depth/stencil state. Recording a view does not increment the widget skip
count or enqueue another pass.
