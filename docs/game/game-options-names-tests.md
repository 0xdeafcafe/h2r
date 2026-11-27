# Game options: names and tests

Names with any character, tests and open items. Part of [game-options.md](game-options.md).

## 4. Names with any character

Status: Design. Nothing in this section is built; the first subsection
records the retail behaviour as read from the XBE.

### What limits a name (confirmed unless marked)

The keyboard screen opens with a context (0..20) that sets its title, its
length and its check on Done (`0x238114`, flags table `0x44a9f8`):

| Context | Name | Length | Check on Done |
|---|---|---|---|
| 0..4 | profile name | 16 (cut to 17 units with the NUL when it opens) | **letters, digits and spaces only**; must not already exist |
| 5..7 | variant name (7 also cuts 2 units on open) | 16 | **letters, digits and spaces only**; unique per type |
| 8, 9 | playlist name | 16 | one letter or digit |
| 10, 11 | saved film name | 16 | one letter or digit |
| 12, 13 | gamertag, clan-mate gamertag | caller's | the gamertag rule (`0x238ce9`) |
| 14 | message | caller's | may be empty |
| 15 | squad (session) name | 16 | may be empty |
| 16 | clan name | 15 | letters, digits and spaces only |
| 17..19 | clan description, motto, (inferred) URL | 150 | may be empty |
| 20 | ours (kit text rows, invites) | caller's | none |

- "Letters, digits and spaces" is `iswalnum` / `iswspace` (`0x321e23`,
  `0x321e15`), which refuse every unit from U+0100 up and all punctuation:
  "Wheeeeee!" can't be typed as a variant name, and (inferred) our
  community built-ins with punctuation can't be copied under their own
  name.
- Contexts 0..7, 12, 13 and 16 also hide the accents and symbols pages.
- The key handler maps U+E000..U+F8FF to U+25A1 (`0x140420`); INSERT
  (`0x238932`) checks only length. `keyboard_glue` (host typing) copies
  that mapping and drops controls and anything past U+FFFF.
- The text scanner reads `|` as an escape (`||`, `|n`, `|t`, alignment;
  `0x13fbd0`): a name with `|` draws differently from its bytes. The retail
  keyboard has no `|` key.

Other named things:

- **Gamertag:** `live_proto::Gamertag`, 1..15 ASCII letters, digits and
  single spaces, first a letter, case-insensitive key. Enforced by the
  server, the worker, the HLE sign-in and the game (contexts 12 and 13).
  The XOnline structures hold it as 8-bit bytes (`live-hle` `state.rs`,
  `invites.rs`).
- **Clan (team):** the server allows any text under 16 UTF-16 units, trimmed,
  no NUL (`live-proto/src/team.rs`); the game's keyboard is the narrow part.
- **Map names:** the content package's display name is free text, cut to 31
  units in the map list (`content/src/map_list.rs`); the map id is ASCII
  (`is_map_name`, `[A-Za-z0-9_-]{1,64}`), a file and folder name. Forge has
  no named maps.
- **Hopper titles and descriptions:** UTF-8, 15 and 127 bytes.
- **Screenshots and films:** theater is not built; films are named by
  context 10 or 11.
- **Ours:** the kit's text rows and the sign-in prompt `.trim()` what comes
  back; Studio's variant name allows 31 units (the game's keyboard then cuts
  it to 16 on rename) and names its export file `{name}.h2variant`
  unsanitised; Studio's ImGui labels treat `##` in a name as an id
  separator; its hopper title clip counts characters where the format counts
  bytes.

### What the fonts can draw

An audit of the disc's fonts (run locally with the font crate) counted, for the UI fonts
that draw names (`conduit` and `handel_gothic`, all sizes; which slot draws
which field is not established, [fonts.md](../formats/fonts.md)), the units that have a glyph of
their own:

| Range | Drawable in every UI font |
|---|---|
| ASCII U+0020..U+007E | all 95, including the `` ` ``, `|` and `~` the keyboard lacks |
| Latin-1 U+00A0..U+00FF | all 96 |
| Latin Extended-A U+0100..U+017F | 10 of 128 |
| Latin Extended-B | 1 of 208 |
| Spacing modifiers | 9 of 80 |
| Greek | 1 of 144 |
| Cyrillic | none |
| General punctuation U+2000..U+206F | 15 of 112 |
| Currency | 1 of 48 |
| Letterlike | 2 of 80 |
| Arrows | none |
| Mathematical operators | 13 of 256 |
| Geometric shapes | 1 of 96 |
| Miscellaneous symbols | 6 of 256 |
| Dingbats | 2 of 192 |
| Private use U+E000..U+E0FF, U+E100..U+E1FF | 90, 50 (the button and HUD icons) |
| CJK, kana, Hangul | none (the Asian builds swap in fonts that have them) |

About 394 units in all, plus the icon glyphs `boot::icons` adds. The
`arial` fonts carry more (about 1,170: all of Latin Extended-A and part
of Greek and Cyrillic) but are not known to draw names. U+2699 (the bot gear,
bots.md section 8.2) is in none yet.

### The design

1. **One drawable set.** `font::coverage(&Font)` returns the units with a
   glyph of their own. The name set is the intersection over every font that
   can draw a name, after our patches, minus controls, surrogates, `|`
   (`|` is not a name character) and the scanner's alignment units (U+E405..U+E407).
   Computed at boot from the user's fonts, never shipped. **No new glyphs**
  : the set is what the game's fonts already draw (about
   394 units, above) plus the icon glyphs; nothing is rasterised or drawn
   for names.
2. **The Done check.** A wrapper on `0x238114` replaces the
   letters-digits-spaces rule (flag 2) with "every unit is in the drawable
   set" for profile, variant and clan names; the rest of the check
   (non-empty, uniqueness, the gamertag rule for contexts 12 and 13) runs
   as retail.
3. **Lengths**: gametype names take up to 31 characters, the
   record's whole field. The keyboard's cut to 17 units on open
   (`0x2381de`) becomes 32 for the variant contexts 5..7, and context 7's
   two-unit cut goes. Every other name keeps retail's length.
4. **The accents and symbols pages** are shown in every context (the
   gating at `0x237a46`).
5. **Characters the keyboard has no key for** (`` ` ``, `~`, the Latin-1
   letters missing from the accents page, the symbols, the icons): a kit
   page "More Characters", a grid of the drawable set by block, opened from
   the keyboard (a free button), inserting through INSERT as
   `keyboard_glue` does. Host typing already reaches INSERT; its filter
   becomes the drawable set.
6. **Icons in names**: allowed, in the game and on the site.
   The U+25A1 mapping (`0x140420`, and its copy in `keyboard_glue`) is
   lifted for names. The site draws them from the user's own disc: the
   glyph images are exported at asset-extract time (the
   `h2 live assets extract` game-art path) and served like the other game
   art, never committed; without the art the site shows a neutral
   fallback. Any other glyph the site needs comes from the same export.
7. **The bot gear** (U+2699, bots.md section 8.2) stays as that design
   has it: a glyph patch of its own, untouched here.
8. **Byte for byte.** The game's text is UTF-16 and BMP-only. Names stay
   as typed through the save (signed by the game; directories are hashed,
   so no host path holds a name), the record, the wire and telemetry
   (`String::from_utf16_lossy`, which loses only lone surrogates, which
   the drawable set excludes). Fix the places that change names: drop the
   kit's and sign-in's `.trim()` (refuse leading or trailing spaces
   instead), sanitise only Studio's export file name, escape `##` in
   Studio labels, and count bytes in Studio's hopper clip. On the site,
   stop `text-transform` on names (it turns ß into SS on screen).
9. **Map names:** display names follow the same set, checked by the map
   builder with a warning; ids stay ASCII (they are file names).

### Gamertags and display names

Gamertags stay ASCII: a gamertag is a login, a case-insensitive key, a
stream name, a URL path and an 8-bit field in the XOnline structures the
game reads. Each account gets a **display name** beside it, which can hold
any character (BMP, the drawable set when typed in the game; the server
stores what it is given, byte for byte). It is a Live server change:

- **Both homes.** The native server and the Worker store and serve it the
  same way, behind the existing account store traits, with a parity test
  (docs/live, "two homes").
- **API.** Account create and update take it; every response that carries a
  player for people to read (rosters, games, leaderboards, friends, the
  stats endpoints) carries it next to the gamertag, which stays the key.
- **Game.** The HLE writes the display name into the player's UTF-16 name
  (player `+0x44`, which the lobby, scoreboard and carnage report draw)
  where it writes the gamertag today; the XOnline 8-bit fields keep the
  gamertag.
- **Site.** Shows the display name wherever a name is shown, the gamertag in
  URLs and as a secondary line; icon glyphs drawn from the exported art.
- **Rules.** 1..16 UTF-16 units, no controls, no surrogates, no `|`; no
  uniqueness (the gamertag is unique). Impersonation is the risk a
  display name brings: the site shows the gamertag beside it on profile
  pages.

## 5. Tests

`cargo test -p variants`, `cargo test -p hopper-edit`, `cargo test -p studio`,
`cargo test -p boot --lib` (community_variants, variant_options, variant_ext,
skulls_custom, string_ids) and `cargo test -p boot --tests` (the plan
goldens).

The record:

- `variants` `record::tests::the_cleanup_keeps_values_the_editor_never_offers`
  (weapon 1, 15 and 20, respawn 0, score, time, lives and penalties at
  0x7fff and team count 8 pass `record::check` and round-trip; one past each
  limit is refused) and `the_cleanup_ignores_the_spare_bytes` (bytes written
  to the spare ranges pass `record::check` and round-trip, and no setting
  the writer knows lives there);
- `record::respawn_zero_with_cycling_is_refused`,
  `record::every_offered_team_count_is_kept`.

Our added choices:

- `extra::every_added_choice_is_one_the_game_keeps`,
  `added_choices_go_in_value_order_and_never_twice`,
  `the_schema_shows_ours_beside_the_discs`;
- `variant_options::our_choices_join_the_retail_lists_in_value_order`,
  `a_list_the_page_cannot_hold_stays_retail`,
  `every_added_choice_has_a_label_id_of_ours`;
- `community_variants::bumper_cars_needs_our_none_and_bounce_practice_has_a_magnum`;
- `schema::weapon_tests::{the_unoffered_weapons_are_read_from_the_globals_list,
  a_short_or_missing_list_gives_nothing, weapon_labels_come_from_the_tag_path,
  the_weapon_slots_join_both_weapon_settings_in_order_once}`.

The extension block:

- `ext_weapons::{ids_are_unique_nonzero_and_paths_canonical,
  a_picker_step_and_an_id_round_trip,
  paths_are_found_in_either_slash_and_case, names_say_what_an_id_is}`,
  `ext::a_weapon_id_the_table_lacks_ignores_the_block`,
  `panel::the_other_weapons_edit_per_slot_and_undo`,
  `variant_weapons::{the_calls_slot_is_told_by_its_return_address,
  only_weap_tags_are_kept_and_paths_match_in_any_slash_and_case,
  a_slot_is_answered_with_its_own_weapon,
  nothing_asked_nothing_known_or_no_tag_leaves_the_routine}`,
  `variant_page::tests::the_weapon_rows_pick_from_the_table_and_write_the_block`;
- `ext::crc_matches_zlib`, `the_default_is_all_zero_and_reads_as_none`,
  `a_block_round_trips`, `a_damaged_block_is_ignored` (every byte flipped
  in turn), `out_of_range_values_are_ignored_even_with_a_good_crc`,
  `a_damaged_block_shows_as_an_issue`;
- `record::the_extension_block_survives_the_cleanup_and_every_setter`,
  `save::a_saved_file_keeps_the_extension_block`,
  `portable::the_extension_block_travels_in_a_portable_file`,
  `panel::extension_options_edit_and_undo_per_option`;
- `hopper-edit` `model::a_hopper_slot_carries_the_extension_block`; the
  `studio` `app::tests` variant actions test sets and undoes it.

The page and the game rules (`boot`):

- `variant_page::the_edited_record_starts_at_0x54e4a0` (and the ignored
  `edited_record_is_where_the_xbe_says`),
  `our_row_follows_the_skulls_row_and_grows_the_list_once`,
  `a_press_opens_the_page_on_the_editors_stack`,
  `the_page_edits_the_records_block`, `strings_are_ours_and_distinct`,
  `the_row_and_the_page_are_on_the_plan`,
  `the_grenades_row_toggles_the_flag`,
  `the_team_count_row_edits_the_records_field`,
  `the_scale_rows_step_their_percents`,
  `option_rows_read_and_write_their_own_bits`,
  `a_row_per_option_after_the_fixed_rows`,
  `a_step_writes_the_option_bits_and_keeps_the_extras`,
  `a_record_with_no_script_has_no_option_rows`; `ui_kit::screens` checks
  the page (`variant-more-options`);
- `variant_ext::alone_or_hosting_applies_system_link_and_guests_dont`,
  `reads_the_active_records_block_in_multiplayer_only`,
  `grenades_are_topped_by_a_lone_machine_or_the_host_never_a_guest`,
  `damage_shield_and_health_scale_the_amount`;
- `skulls_custom::a_variants_gravity_composes_with_low_gravity_on_physics_memory`,
  `skulls_custom::tests::a_variants_unlimited_grenades_reach_every_players_unit`.

Live transport:

- `live-proto` `session::variant_options_are_optional_checked_and_updated`,
  `presence_carries_a_build_and_capabilities_checked_and_optional`,
  `lacking_names_the_players_whose_build_cannot_and_skips_the_unknown`;
- `live-server` `sessions::sessions_carry_the_hosts_variant_options`,
  `presence_privacy::build_and_capabilities_show_to_everyone_and_are_checked`;
- `live-hle` `server::guests_read_the_variant_options_their_host_advertises`,
  `server::presence_says_what_a_build_can_do_and_the_host_names_older_players`;
- `boot` `variant_ext::a_live_guest_applies_only_what_its_host_sent`,
  `the_key_is_the_name_and_engine_only`,
  `a_guest_waits_for_its_hosts_key_and_says_why_not`,
  `a_guest_with_its_hosts_block_applies_it_in_game`,
  `the_host_names_its_guests_from_the_sessions_players`.

## Not verified

Only a game run shows these.

- What the game does for an unarmed player (Bumper Cars, starting weapon
  None): melee, the HUD, picking a weapon up.
- Respawn 1 s: back in after about one to two seconds (retail's floor and
  its 32-tick slot).
- What the game's editor shows for a storable value it doesn't list.
- Whether the game's own editor copies the spare bytes when it edits and
  saves a variant (a Studio-written gravity surviving an in-game edit).
- Gravity 50% in a custom game on Live: both machines jump the same height
  with no rubber-banding; with the Low Gravity skull a quarter of normal on
  both; the same variant over System Link plays normal gravity.
- An older-build guest of a new host: how its game feels (the desync above).
- Damage, shield and health scales: a hit takes the share the scale says,
  vehicles take normal damage, and a guest's hit markers and kills agree
  with the host.
- Team count 3 on a Team Slayer on a map with more teams: what the lobby's
  team switch offers, and whether guests see three teams.
- Weapons 15, 16 and 20: picking each in Studio, saving and playing spawns
  with it.
- Our weapons: each path of `ext_weapons::TABLE` exists as a `weap` tag on
  the multiplayer maps that should have it (a map without one says so in the
  log and spawns the record's weapon); a Live host's guests see the unit
  with the weapon.
- The unoffered vehicle values and setting 80.
- Which font slot draws names in each place (the design assumes `conduit`
  and `handel_gothic`).
- The unit's grenade counts; the damage path after the steps (`0xd82e0` and
  the damage report read `+0x1c`).
- Readers of the spare bytes through the session copy (`S+0x4db0`) and the
  pending options (`0x4ed3a8`): a full scan.
- Host migration: a new host holds only the wire fields, so it would need
  the block from the session as a guest does.
- The names design (section 4), all of it.

## Related

[community-gametypes.md](community-gametypes.md), [skulls.md](skulls.md),
[../editor/variants.md](../editor/variants.md),
[../input/keyboard-charset.md](../input/keyboard-charset.md),
[../live/live-gamestate.md](../live/live-gamestate.md).
