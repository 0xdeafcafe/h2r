Evidence for the normal forms and the asset catalogue of
[asset-store.md](../../engine/asset-store.md) ("Normal forms", "The
catalogue") and [tag-repo.md](../tag-repo.md) ("The catalogue"): a store's
variants by group as the files it keeps and as the assets it holds.

## 2026-11-24: invented stores; the disc's assets are not counted

Commit: branch `store-as5`, `21eafd5d0`, from `12ae05214`. Method: the
unit tests on invented tags; no disc was read for this entry.

A store's variants by group as files are the whole-disc extraction of
2026-11-17 ([tag-repo.md](../tag-repo.md), "The whole disc"): 22,430 keys,
172,129 tags, 86,219 of them kept as variants, 57,517 of 65,309 sounds and
18,516 of 25,214 bitmaps. The same store by asset hash is not counted:
`h2 tags catalogue --repo <data>/tags` on a store `h2 build` made prints
it, one line per group that has variants.

The invented store of `crates/formats/tags/src/repo/tests.rs`
(`the_catalogue_lists_each_asset_by_hash_and_is_rebuilt_to_the_same`):
three maps; two of them hold the same two sounds the other way round, the
third the first sound with another sample; each map a gestalt, a globals
tag of its own and a weapon that names the first sound. A sound's file is
its 20-byte header, so the third map's copy of the first sound is the
first map's file although its map plays other samples.

| Group | Keys | Files | Keys with variants | Readings | Of another file | Assets | Keys with more than one | Of another asset |
|---|---|---|---|---|---|---|---|---|
| `snd!` | 2 | 4 | 2 | 6 | 2 | 3 | 1 | 1 |
| all | 7 | 11 | 4 | 15 | 6 | 10 | 3 | 5 |

A reading is one map's entry for a key; "of another" counts the readings
whose content is not the first map's for the key (the shared caches come
first, then the maps by name). The sound held at two indices in two maps
is one asset read from two files; the same sound with another sample is
another.

Reproduce:

```sh
cargo test -p tags --lib repo::tests::the_catalogue -- --nocapture
h2 tags catalogue --repo <data>/tags        # after h2 build; --force rebuilds a current catalogue
```
