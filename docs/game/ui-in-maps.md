# Our screens as tags in the maps

Status: Partly built. The tags are generated, added to the maps, listed in each map's `wgtz`, cached and opened by the list classes on their own id, all tested offline; the levels' maps, text and picture blocks in the tag, and the dialog classes are not moved (see "What is not in the tag").
Code: `crates/extensions/game/src/ui_kit/inject/` (`mod.rs` the plan and ids, `tag.rs` the tag and the map edit, `rewrite.rs` the VFS rewrite and its cache), `crates/extensions/game/src/list_class.rs` (`load_with`, `served_tag`). Tests: `ui_kit::inject::tests`, and in `list_class` the loader tests named below.
Switch: none.

Every screen in `ui_kit::screens::NAMES` is a `wgit` tag in the maps the game reads its screens from, listed in that map's `wgtz` under a screen id of ours, so the game finds it by id as it finds a retail screen ([ui.md](../formats/ui.md), "The tag graph"). The tags are made at boot from the user's own map and our docs ([ui-builder.md](ui-builder.md), "Screens in YAML"); nothing of a map is in the repository. A list class opens on its tag instead of a per-open copy of the Controls tag.

## How it works

1. **Plan** (`inject::plan`). For each name in `NAMES`: its doc says the kind (`list`, `dialog`, `code_entry`, `picker`, `hud`) and layout name; its sample (`screens::sample`) says the list place (`ListPlace`) and animation style. The entry has the screen id (`ID_BASE + index in NAMES`, `0x4000` up; retail's ids are far below), the tag name (`ui\screens\h2recomp\<name>`, a dot written `_`), and a shape: `List` (the Controls screen's tag as `copy_tag` made it), `Dialog { base }` (the base dialog as `write_base_tag` made it) or `Bare` (a HUD: a pane with the template's flags and nothing else).
2. **Tag** (`inject::tag::screen_block`). The template is a retail screen of the same map, found in its `wgtz` by id: Controls (39) in `mainmenu`, the in-game Controller Settings (200) in the shared maps, a dialog's base (222, 223, 240). The tag is a `map::build::Block` tree: the template's root (`0x68` bytes) with our id at `+4`; one pane (the template's `0x4c` bytes, the pane animation set when the style asks) whose list block is the template's, placed by the layout (skin, visible count, anchor) and given the style's track; and the pane's bitmap block holding the frame art left of the layout's edge (at most four). The pane's other blocks (texts, buttons, tables, model scenes) are the template's, by address: a template tag is pinned, so its blocks keep their addresses in the rebuilt map, and the new tag points at them as retail tags point at each other's blocks ([ui.md](../formats/ui.md), "Aliased blocks"). The strings reference is the template's, copied with the root.
3. **Map** (`inject::tag::add_screens`). The map is decomposed (`map::build::TagSet::from_map`, every tag where it was), the index given room for more tags (as `campaign_cards::add_cards` does, and for the same reason the tag-memory bound is lifted for a few kilobytes), one `wgit` added per entry (`TagPayload::Built`), and each `wgtz`'s screens block laid out again past the pinned metadata (`append_pinned_blocks`) with its old entries first and ours after, its count and address patched in the pinned `wgtz`. The lookup takes the first match, so a retail screen is found as before. The map is built again; its checksum is the builder's. A screen whose template is not in a `wgtz`, or whose layout names a skin the map's `wigl` lacks, is left out and named; if none can be added, the map is served unchanged.
4. **Rewrite** (`ScreensRewrite`, a `vfs::Rewrite`). `boot` chains it after the level-picture rewrite, so it decomposes what that produced. It wants `maps\mainmenu.map`, `maps\shared.map` and `maps\single_player_shared.map` (`rewrite::MAPS`).
5. **Open** (`list_class::load_with`). A list class asks the game for its id (`SCREEN_TAG_BY_ID`, `0x148098`); when the interface holds it, the loader runs the generic screen constructor with that id (`construct_on`) and adopts the screen as before. The screen-tag function (`0x22f871`, hooked in `layout_glue`) finds the tag by the id at the widget's `+0x70`, and `list_class::served_tag` gives back what the lookup found for an injected id instead of making a copy. When the id is not found (a map served without our screens) the loader builds on the base screen and the copy path serves it, as before.

### The rewrite and its cache

The rebuilt map is kept per map at `<data>/cache/ui-screens/<map>.map` with a receipt `<map>.json`. The key is the SHA-256 of the input map's bytes, the docs (`Docs::hash`: `assets/ui/design.yaml` and every screen's file as baked into the build, in name order), the plan (the entries as JSON: layouts and styles come from Rust) and `rewrite::VERSION`. A boot after the first reads the file back, checks its digest against the receipt and serves it; a changed doc, layout or generator builds again. The `.map` file is the map itself, so the editor and `h2 ui --all <file>` read it as any loose map.

`h2 ui inject [--map mainmenu|shared|single_player_shared] [--disc disc.iso] [--data DIR]` builds or reads the cached map for a disc map, prints the cache path and the screens added or left out, and runs the `h2 ui --all` checks on it.

### Editing

The editor opens the cached `mainmenu.map` as a loose map (Maps, the file), and its Tags tool lists our screens as `wgit` tags beside the retail ones, edited the same way. Moving a widget of one of ours writes a field of that tag in the map's tag document, as it does for a retail screen ([ui-builder.md](ui-builder.md), "Editing in the editor"). The editor writes a patch against the map it opened, so a patch made on the cached map applies to it and not to the disc's.

## Interfaces

- `inject::id_of(name)`, `is_injected_id(id)`, `tag_name(name)`, `list_id(name)` (only a `list` doc opens on its tag).
- `inject::plan() -> Vec<Entry>`; `add_screens(map_bytes, &plan) -> Added { map, injected, skipped }`.
- `ScreensRewrite::{baked, new(&Docs), with_cache(data_root), rebuild(stem, bytes), cache_file(stem), key(stem, bytes)}`.
- Guest: `0x148098` (screen by id, called by the loader), `0x22f871` (the hook leaves an injected id's tag alone), `0x230451` and `0x2c84b8` (the Controls constructor's two steps, run by `construct_on`).

## Rules and limits

- Nothing disc-derived is stored in the repository: the tags are built from the user's map at boot and live in the host cache and the served map.
- A screen's id is `ID_BASE + its index in NAMES`; adding a screen changes later ids and the plan, so the cache rebuilds. Ids are never saved.
- A tag has no text of its own yet. The tag's text blocks and header keep the template's string ids, and the strings our screens show are served by id from the kit's locale source as before.
- A screen is injected only where its template is: the list classes in `mainmenu` (Controls) and the shared maps (in-game Controller Settings). `Entry::templates` orders them.
- The rewrite runs only on files the VFS rewrites (at most `vfs::MAX_REWRITE_BYTES`); a larger map is served as it is and the classes use the copy path.

## What is not in the tag

- **Text and picture widgets a class adds** (the description, panel texts, the picture and frame art, the footer legend) are still blocks of ours built by the class after the pane is set up; moving them into the tag means removing the class's constructors for them and finding the widgets by their tag index. Their strings are set at run time by `TEXT_SET`.
- **A `unic` and string ids of our own.** A text block's string id must be in the map's string table and resolve through the screen's `unic`; our ids (`ui_kit::ids`) are in neither, and are answered by the locale source. A tag with its own texts needs strings interned in the table and a `unic` (or an id-to-text registry the locale source reads, filled by the rewrite); see [localization.md](../formats/localization.md).
- **Dialog classes** (`kit.dialog*`, `kit.picker*`, the sign-in prompt, the Forge dialogs) still take their copy (`dialog_tag`): their tags are generated (`Shape::Dialog`: the base's root, a pane, the placed list, the panel art, no text block) so the editor shows them, but a dialog's body, cells and status are text blocks whose strings the class supplies when it opens.
- **The level maps.** A game's interface is chosen by the scenario's type (`0x1482e8`, `matg +0x110`, `+0x84`, `+0x8c`, `+0x94`), and the maps that hold the wgtz a game reads are not decided by reading code: see "Not verified".
- `copy_tag` and its scratch area stay for the screens opened where no tag of ours is found, and for the classes above.

## Evidence

- **E** `0x148098` walks `wgtz +8` comparing `wgit +4` ([ui.md](../formats/ui.md)); `0x22f871` maps a screen widget's id (`+0x70`) to its tag ([engine-map.md](../engine/engine-map.md)).
- **E** The wigl and the wgtz per scenario type: `0x1482e8` picks the `wgtz` at `+0x84` (main menu, type 2), `+0x8c` (type 0) or `+0x94` (type 1) of the interface block at `matg +0x110`.
- **V** (read at the disc, [ui.md](../formats/ui.md), "Validation") every map has exactly one `wigl`; most have one `wgtz`; `shared` and `single_player_shared` have two, both naming the map's `wigl`; the Controls screen (39) is in `mainmenu` only, the in-game Controller Settings (200) in `shared`, `single_player_shared` and the levels, the large dialog (240) in `mainmenu`, `shared` and `single_player_shared`.
- **Code** `list_class::construct_on` is the Controls constructor's steps on any screen id; with 39 it is that constructor.
- **Test** `ui_kit::inject::tests::generated_screens_decode_and_pass_the_ui_cross_checks`, `..::a_list_screen_s_tag_is_the_copy_the_class_used_to_make`, `..::the_rebuilt_map_round_trips`.

## Not verified

Only a game run, or the disc, can show:

- That the game opens each injected list screen on its own tag and shows what the copy showed (rows, list place, frame art, animation), in the main menu and in a game.
- That the string and unic resolution of a screen whose tag is ours behaves as the Controls screen's (the root keeps the template's strings reference).
- Which maps hold the `wgtz` a game's interface uses: whether a level's `matg` names its own or the shared map's. `h2 ui --all all --disc ...` prints counts only; reading each map's globals interface block (`h2 tag <map> globals\globals matg`) and the tag-index map of each `wgtz` settles it. Until then the levels are not rewritten and an in-game screen opens through the copy path unless `shared` or `single_player_shared` carries the one the game reads.
- That a game accepts a map whose `wgtz` screens block moved past the pinned metadata and whose index grew (the builder's checksum and the cache copy's compare at `0x214616` pass offline).
- Boot cost: the rewrite reads and hashes each wanted map at its first access, even from the cache.
- That the editor's Tags tool lists, reads and saves a field of a tag of ours on the cached map.

## Related

- [ui.md](../formats/ui.md): the tag graph, the `wgit`, `wgtz`, `wigl` and `skin` layouts and the `h2 ui --all` checks.
- [ui-builder.md](ui-builder.md): "The runtime tag recipe", "Screens in YAML".
- [campaign-picker.md](../halo1/campaign-picker.md): "Pictures", the rewrite this one is modelled on.
- [map-builder.md](../formats/map-builder.md): the builder and pinned tags.
