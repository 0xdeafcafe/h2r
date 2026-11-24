# List scroll arrows and button selection

Retail Xbox executable, worktree base `4d40837d1`, 2026-11-24.
Reproduce each read with `h2 disasm disc/default.xbe ADDRESS COUNT`.
Use 180 instructions for either draw and 60 for its helpers.

| Address | Evidence |
|---|---|
| `0x0024bc12`, `0x0024c0c4` | The layout routine visits the linked item widgets. The row binder writes each child's row datum at `+0x70` and advances along `+0x18`. The item and its animated highlight children have their own entries in the sorted list. |
| `0x0024bda2`, `0x0022f0ff` | Either low bit of the byte at `+0x42` suppresses the arrows. The draw resolves the active pane's first list (`0x0024bbf3`), its signed skin index at `+4`, and the skin through the shared interface's skin references (`0x0014837a`). |
| `0x0024c63e`, `0x0024c676` | The first child at `+0x14` supplies the upper bound for the backward row search. The last sibling supplies the lower bound for the forward search. A missing child, array or bound row suppresses the corresponding arrow. The backward iterator (`0x0016bcc0`) tests row salts. The forward iterator (`0x0016bbc0`, `0x0016bc00`) tests the array's occupancy bitmap. |
| `0x0024be2c`, `0x0024bf2d` | The two paths pick alpha from the corresponding state word (`+0x74`, `+0x76`). The skin's bitmap datum is at `+8`, with image 0 for up and 1 for down. Skin points at `+0xc` and `+0x10` add to the animated list rectangle's left and upper edge. Helpers truncate the animated offset to signed words for these arrows. |
| `0x00253cc8`, `0x00253ea8` | The button indexes the first pane's array of 0x3c-byte blocks by its signed word at `+0xf8`. The optional bitmap datum is at block `+0x28`. The signed point at `+0x2c` adds to animated left and subtracts from animated top, both float coordinates. Bitmap alpha comes from widget `+0x64`; the draw calls the text interface afterward. Debug labels take a separate branch at `0x00254027`. |
| `0x00254173`, `0x0022ed7a`, `0x00148d91` | Standard button slot `+0x44` chooses image 1 when the bitmap has multiple images and the focused widget's parent chain includes the button. The channel's focus is at `+0x34`; the screen's virtual getters return channel and user from `+0x74` and `+0x78`. |
| `0x0023618e`, `0x002360c3`, `0x00235e5e` | Two corners receive the depth projection, X translation and Y inversion. Their source UV span uses the image width and height at image `+4` and `+6`. The packed tint uses the rounded alpha and white RGB. The bitmap descriptor selects blend mode zero through `0x000142f0`. |
| `0x0023675a`, `0x00012360`, `0x00052040` | Texture lookup before submission can start a resource request. Only the path with a newer residency stamp and a nonnull resident pointer returns directly. The submit routine also checks the word at `0x00485602`. |

The reader accepts full-canvas art with a resident texture. Custom screen
tag lookup and repositioned list arrows use extension-owned vtables and
stay on the guest path. A failed screen lookup can record a missing ID at
`0x00148112`, so the reader preserves that call on a miss.
