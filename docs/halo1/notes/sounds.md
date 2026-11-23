# Sounds: measurements

Evidence for [../sounds.md](../sounds.md).

All numbers come from the ignored tests named in the entries, run against
the owner's Halo 1 and Halo 2 discs.

## 2026-11-20: promotions, mouth curves and moved sounds

Commits: `0cc737264` (promotion census), `fe15711b8` (mouth curves),
`51b38ca73` (moved sounds). Method: `every_retail_halo1_promotion_round_trips`,
`a10_dialogue_mouth_curves_reach_the_retail_resource_layout`,
`every_retail_sound_merges`, `a_transplanted_tree_takes_its_sounds`.

- A read-only census of the 23 playable Halo 1 maps found 253 redirect
  rules and 77 rate limits.
- On a10, 1,038 sounds contain 4,659 authored mouth curves.
- A read-only survey of 30 retail Halo 2 maps found 231 promoted sounds
  among 65,309 sound headers. Every promotion had one owner, one timer per
  rule and sound-relative targets inside its owner.
- 04b's flood combat elite transplanted onto 05a moves 42 sounds.

## 2026-11-19: campaign loops

Commit: `a2031f48e`. Method: the ignored campaign disc audit.

Source loops per level, in campaign order from a10 to d40: 50, 55, 47, 49,
62, 55, 35, 66, 51 and 78. Their distinct sample spans range from 41.9 MiB
on c20 to 104.2 MiB on d40.

## 2026-11-18: class distance flags

Commit: `81a8f1303`. Method: `distance_and_cone_flags_follow_the_retail_rule`
(`tests/fields_disc.rs`) on every map of both discs.

Halo 1 distance 0 (the class default) is on 13,149 of 15,698 Halo 1
sounds. On all 30 Halo 2 maps, 65,309 sounds: every sound whose playback
distances are 0 has flags `0x400` and `0x800`, no other has either, and
every one has `0x1000`.

## 2026-11-18: what a10's opening cinematic plays

Commit: `3f13910f0`. Method: `level_loops_and_their_character` in
`tests/looping_disc.rs`, `$H1_LEVEL` default a10.

The cinematic `x10` starts `sound\sinomatixx\x10_music01` (one stereo
44.1 kHz track, 114 s, flagged "not a loop"), then on the bridge and in the
hangar `x10_foley1..3` and `x10_music02..04`, all "not a loop" with one
track. The level's one background sound, `sound\sfx\ambience\a10\shipambience`
(a `battleship` drone, `hf` 0.02, at -6 dB, and `shipcreaks` details), is
named by every cluster of the ship's BSPs and the hangar's, none of the
space BSP's (`a10_space`: one cluster, -1). No sound is a drone where Halo
1's is not. The repeated whirr was a script: Halo 1's `(sleep -1)` did
nothing in Halo 2, so the continuous `dialog_flavor_captain` started eight
captain lines every tick (fixed in converter version 67).

## 2026-11-17: the whole-disc census

Commits: `e7d7ead12` (decode, proof), `65a4321b0` (looping),
`0daf3e54a` (audit). Method: `every_h1_sound_decodes`,
`retail_gestalts_decode_and_rebuild`, `converted_sounds_read_back_sample_identical`,
`every_campaign_sound_plays_what_it_was_made_from`, `tests/looping_disc.rs`.

Layout: beavercreek's samples span `0x16e7200..0x22ab698`, tag data from
`0x22ab800`. 34,315 permutations on the Halo 1 disc have mouth data and
none have subtitles. 10,807 of 72,558 permutations continue another.

Decode: 24 maps, 15,698 sounds, 72,558 permutations, 61,751 playable,
**23.0 hours**, every one decoded with no error.

| Rate | Channels | Compression | Sounds | Hours |
|---|---|---|---|---|
| 22,050 | 1 | Xbox ADPCM | 14,567 | 18.40 |
| 22,050 | 2 | Xbox ADPCM | 677 | 0.52 |
| 44,100 | 2 | Xbox ADPCM | 454 | 4.14 |

53 sounds have more than one pitch range (engines); 258 promote.

Halo 2 gestalts: retail ranges with one permutation have `+4 = 0`,
`+6 = ffff` on all but 598 of 16,942. Of the multi-permutation ranges,
19,183 use `+4 = ffff` with `+6` a bit offset into block 7, and 29,978 use
`+4 = 0` with a `+6` that is not such an offset. 6,904 of 310,088 retail
chunks have bit 31 of their size set: 4,358 are a permutation's only chunk,
2,487 its first or last, 59 in the middle, and none is `0x7ff8` long. A
campaign base holds up to 19,963 chunks.

Read-back proof (`converted_sounds_read_back_sample_identical`):

| Source | Sounds | Permutations | Samples | Audio |
|---|---|---|---|---|
| beavercreek, every sound | 316 | 902 | 21,532,928 | 837.7 s |
| a30 dialogue (classes 19, 44, 46, 47; every 4th) | 257 | 1,283 | 36,806,272 | 1,669.2 s |

Audit: a10 on 05a_deltaapproach, 3,988 sounds: 175 from Halo 1 (602.4 s),
3,813 the base's and donors', 14,667 permutations, no problem. Looping:
734 `lsnd` and 63 `snde` on the Halo 1 disc all convert.

Sound counts per Halo 1 class that Halo 2 lacks: 15 slow_particle_impacts
63, 27 device_force_field 10, 29 device_nature 0, 30 device_computers 31,
35 ambient_computers 0, 39 first_person_damage 115, 44 scripted_dialog_player
913, 45 scripted_effect 468, 46 scripted_dialog_other 0, 47
scripted_dialog_force_unspatialized 33.
