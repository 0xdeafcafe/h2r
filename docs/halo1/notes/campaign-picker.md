# Campaign picker: measurements

Evidence for [../campaign-picker.md](../campaign-picker.md).

## 2026-11-18: the disc's mainmenu under the card rewrite

Commit: `98286215d`. Machine: the owner's Mac, the owner's Halo 2 disc.
Method: `H2_DISC=disc/halo2.iso cargo test -p game --lib
the_disc_s_mainmenu_takes_ten_cards -- --ignored` (ten cards into the
disc's `mainmenu.map`).

Every retail tag unchanged, each card a 220x207 `0x2d480`-byte LOD of
the scaled pixels, the checksum good, tag memory grown by one page (two
is the test's bound), the map 61.6 MB of the 64 MiB limit, the rebuild
31 ms.

## 2026-11-18: retail level pictures carry their own frame

Commit: `d14025de1`. Machine: the owner's Halo 2 disc. Method: read
`ui\code_global_bitmaps\campaign0`..`campaign19` of `mainmenu.map`, and
the level select's picture widget through `h2 ui mainmenu load_level`.

The widget is one plain bitmap block (`[177 39 -30 259]`): the screen
draws no frame. Every used picture (`campaign0` to `campaign14`) has
the same alpha, pixel for pixel: 834 opaque border pixels (a one-pixel
rim and the diagonal of the cut, `(154, 171, 189)`, `campaign14` a
little lighter), 136 clear pixels past the cut, and the picture at
alpha 191, so it is drawn a quarter see-through. The free slots' five
pictures are fully clear.

## 2026-11-18: the owner's Halo 1 progress after one launch

Commit: `34263bf84`. Machine: the owner's Mac. `<data>/campaigns/progress.toml`
held `finished = [-1, -1, -1, -1]` and `last = 4097` for Halo 1: The
Pillar of Autumn was started, not finished, and the level select listed
the first level alone.
