# Retail's settings dialogs: where the window's line and a setting's help go

Evidence for [ui-builder.md](../ui-builder.md), "The settings dialog" and "A page's header". Nothing here states what the code does; the design pages do.

## 2026-11-22: tag dumps of the retail settings screens

Read at master `847362549` by the uik-settings planner, read-only, off the user's disc. Rectangles are `[top left bottom right]`, UI units, origin at the screen's centre, Y up.

How to reproduce:

```sh
h2 ui mainmenu 'ui\screens\game_shell\settings_screen\variant_settings\editing_format_screens\quick_options'
h2 ui mainmenu 'ui\screens\game_shell\settings_screen\variant_settings\editing_format_screens\low_level_settings'
h2 ui shared '<tag path of handicap_settings, look_sensitivity_settings, squad_privacy_settings>'
h2 tags shared bitm
h2 tags single_player_shared bitm
h2 tags shared skin
```

| Retail screen | What it shows | What it supports |
|---|---|---|
| `quick_options` (222, flags `0x10`) | bitmap `mp_games2` `[166 -267 126 -227]`; text `quick_options_variant_name` `[163 -224 143 0]`; text `quick_options_help` `[143 -224 123 270]`, one fixed string; list skin 2 at (-254, 87), 11 items; no per-row text | the top is a static icon and a line about the window |
| `handicap_settings` (45), `look_sensitivity_settings` (42), `squad_privacy_settings` (25), in `shared.map` | a `subheader` or `help` text `[72 -122 -19 122]` under the header, one fixed string per screen | the top says what the window is for, in a game too |
| `top_level_settings` (221, the variant editor's settings page) | the header and a 12-row `setting_lists` list; no description text | a setting's help is in its picker |
| `low_level_settings` (223) | text `description` `[74 15 -80 250]`, filled by code from the setting's `sily` tag (`+0x18`, its help string id) | the picker shows the setting's help |
| `squad_settings_dialog` (23, Game Setup, flags `0x10`) | text `change_map_help` `[-40 -241 -173 0]` under the list, set from the focused row's action by the update `0x2bbbf6` | the bottom is the focused item's help, on the same header layout and `fs_dialog` panel as 222 |
| `variant_editing_options` (178) | each category pane's help at `[-109 20 -181 268]`, swapped with the focused row | the bottom is the focused item's help |

Facts for an in-game host:

- `mp_games2` (`ui\screens\game_shell\pregame_lobby\mp_games2`) is in `mainmenu.map` only: `h2 tags shared bitm` and `h2 tags single_player_shared bitm` do not list it, so the eleven icons cannot be drawn in a game.
- `shared.map` and `single_player_shared.map` both carry `ui\global_bitmaps\fs_dialog` and the `setting_lists` skin with its art; skin index 2 is `setting_lists` there (`voice_settings_menu`, screen 53, uses it). `error_dialog_large` (240) is there with its own `help` text block `[74 0 -80 260]`.

The places the settings dialog takes from these: the about line is `quick_options_help`'s own block, one line; the help block `[-133 -244 -173 246]` ends where `change_map_help` ends (y -173) on the same header layout, its left and right on skin 2's label left (-254 + 10) and value right (-254 + 500). A row's cap is 34 high from 1 below its item's anchor, so at Quick Options' anchor (-254, 87) row 0's cap reaches y 120, just under the help line's bottom (123): a second about line would sit on it (`every_state_draws_without_a_disc` found that on Halo 1 Options and Firefight), and moving the list off Quick Options' place makes the disc check report the base's frame art away from its list. Eight rows 26 apart end at y -95, above the help block. These are measured on the kit's metrics; a render (`h2 ui preview clankers --state all --disc <iso>`) and a game run settle them.
