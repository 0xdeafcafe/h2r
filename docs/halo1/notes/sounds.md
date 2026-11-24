# Halo 1 sound conversion: measurements

Evidence for [../sounds.md](../sounds.md).

## 2026-11-17: a10's sounds on the Delta Approach base

Commit: not recorded; the result is first in the docs at `42a1c068e`.
Method: `h1_assets::sounds::audit` through
`every_campaign_sound_plays_what_it_was_made_from`
(`crates/tools/h1campaign/tests/disc.rs`, `H1_LEVELS=a10`) on the owner's
Halo 1 and Halo 2 discs.

Result: 3,988 sounds, 175 from Halo 1 (602.4 s) and 3,813 the base's and
donors', 14,667 permutations, no problem. The sample bytes were not the
cause of a10's repeated "wurrrring" (a script was: [../campaign.md](../campaign.md), Sounds).
