# Forge map variants: the file, the pick and the match

Status: part of [forge-map-variants.md](forge-map-variants.md); see there for status, labels and the index. This page: the file, packages, picks, the lobby, materialising and Live.

### The map variant file

A map variant is a Forge document, in the form a game is played from.

- **Wire.** The bytes are `ForgeDoc::canonical_json`: canonical JSON,
  floats as their bit patterns, at most 8 MiB
  ([forge-design.md](forge-design.md), "The document"). There is no
  second format and no envelope: the name is the document's `title`,
  the author its `author`, the base its `base {map, sha256}`.
- **What it may hold** (`ForgeDoc::tier() -> Tier`,
  `Tier::Variant` or `Tier::Map`, in
  `crates/formats/forge-doc/src/variant.rs`). A variant holds
  `Placement::Add`, `EntryOp`, link, unlink and label ops, and nothing
  else: no recipe but the kit's and the AI ground's, no import slot, no source map, no
  placement set or removal by running index, no tag-field, mesh or
  bitmap op. So a variant
  changes the scenario's placement blocks and no tag, its build is a
  scenario patch, and it fits every map the kit fits. A document with
  more is a **Forge map**: it is picked as a map
  ([forge-design.md](forge-design.md), slice F14), keeps `.h2forge`,
  and is never offered as a variant.
  A variant may also hold one Firefight wave set (`waves`, which builds
  nothing, [forge-firefight.md](forge-firefight.md), "The wave set"), and
  in the design, as the AI ground recipe's class donors only, source
  maps (the same page, "Enemy classes").
- **Play form and session form.** A session's document has the
  `forge_kit` recipe (`forge_doc::KIT_RECIPE`) and a saved variant does
  not ([forge-design.md](forge-design.md), "The kit").
  `forge_doc::play_form(&doc)` drops it and `session_form(&doc)`
  appends it after any other recipe, so the play form of the session
  form is the document. A form that changes the recipes changes the
  identity, so it drops `expect`. Save writes the play form and Load
  opens the session form at a fresh log head. The tier is judged on
  the play form.
- **The name** is 1 to 16 characters (`forge_doc::MAX_VARIANT_NAME`),
  the length of the game's own variant names, so it fits every retail
  text that shows one; it is not blank and has no control character
  (`forge_doc::variant_name`). `ForgeDoc::check_variant` holds a
  document to the variant tier (`Refusal::Tier`) and its title to
  that rule. Two variants of one base map may not share a name,
  compared without case: the store's check.
- **Identity.** A variant's id is the document's
  (`ForgeDoc::id`). The map it makes has the `MapId` of format 2, the
  hash of the base's hash and the document's
  ([maps.md](maps.md), "Identity: `MapId`"). That one value names "this
  variant of this map" everywhere a map is named.
- **On disk.** `<data>/map-variants/<base map>/<file>.h2mapvariant`,
  one directory a base map. `<file>` is the name made safe for a file
  system; the name of record is the `title` inside. `live/<id>` under
  the same directory holds the ones fetched from Live, as the
  game-variant store keeps its Live copies
  (`crates/extensions/game/src/game_variants/store.rs`).
  `forge::files::variants` owns the store (`crates/extensions/forge/src/files.rs`):
  `list(data, map, installed)`, `save(data, doc, replace)`,
  `load(data, map, file, installed)`, `rename`, `duplicate`, `delete`,
  where `installed` is the hash of this machine's map of that name, or
  `None`. A file is named by its stem. `save` writes the play form and
  refuses a name another variant of the map has, compared without case
  (`variant_name_taken`), unless `replace` is set; a rename or a
  duplicate never replaces. Two names that make one stem get `-2`. A
  rename or a duplicate drops `expect`, since the title is in the id.
  `list` gives every file, the Live copies after the saved ones, then
  the packaged ones ("A map variant in a package"), each
  with the `Problem` that bars it and its sentence; `load` refuses with
  the same one. Rename, duplicate and delete act on saved files, and on
  one that has a problem. Files are user output and stay out of Git
  checkouts. `suggestable(data)` lists the files of every map that a
  game variant may suggest: the variant tier, this build's builder, in
  their own map's directory, with their base hash and document id. The
  `.h2forge` store of Forge maps is the same module's
  other half. On the Map variants dialogs a fact row (the page's "No map
  variants", the open variant's "Open now") takes no A, and the footer
  follows the focused row: A Select and B Back on an action, B Back
  alone on a fact
  ([forge-map-variants-ui.md](forge-map-variants-ui.md)).
- **No game bytes.** The document rule holds unchanged: typed values an
  author set, tag keys, digests. A print and a `was` are digests. An
  own entry's position is never written unless the author moved it,
  and then the value is the author's.

### A map variant in a package

A package holds a map variant as a `map-variant` item
([studio-redesign.md](../editor/studio-redesign.md), "Contents"). The
source is the same file, so a variant saved in the game and a packaged
one are one format. Code: `crates/tools/workspace/src/map_variants.rs`,
`forge::files::variants` (`Packaged`, `install_packaged`),
`boot::packages::at_boot`.

```toml
[[item]]
kind = "map-variant"
source = "map-variants/lockout/castle.h2mapvariant"
```

| Step | Where | Does |
|---|---|---|
| Check | `workspace::map_variants::check` (`h2 package check`) | the source's name is `<stem>.h2mapvariant`, the stem letters, digits, `_` and `-`; the bytes read as a Forge document that passes `ForgeDoc::check_variant`; the item names no `tag` |
| Build | `workspace::map_variants::apply` | changes no map: the variant's map is built when a lobby picks it ("Materialising for a match") |
| Boot | `boot::packages::at_boot` | gives the store the `map-variant` items of the packages that are on (`workspace::map_variants`, `variants::install_packaged`) |
| List, load | `variants::list`, `load`, `suggestable`, `duplicate` | a packaged variant is a row of source `Package` after the saved and fetched ones, with the same checks and `Problem`s; its handle is `<package>.<stem>`, which no file stem is |

A packaged variant is used wherever a saved one is, since every reader
goes through the store: the lobby's Map Variant picker and Change Map
(`lobby_mode::{map_variant_list, usable_variants}`), Forge's Map
variants dialog, the pick's build (`map_variant`), the rules gate's
census and a game variant's suggestions. `Source::loads` is true of a
saved and a packaged variant and false of a fetched one.

| Call on a packaged variant | Result |
|---|---|
| `load`, `duplicate` | as a saved one; the copy is a saved file |
| `rename`, `delete` | refused, `variant_packaged`: "That map variant belongs to the package <id>. Duplicate it to change it." |

`list_with`, `load_with` and `suggestable_with` take the packaged
variants as an argument in place of the installed ones; Studio and the
tests call them. A package that is off takes its variants out of every
list. Studio saves a variant as an item of the edit target and never
writes the store's files ([studio-flow.md](../editor/studio-flow.md),
"Forge variants and items").

### How a pick names a map variant

A pick is a map and, optionally, a variant of it: the map as the game
names it, and the format 2 `MapId`.

| Who picks | Holds the variant as | Where |
|---|---|---|
| The custom-game lobby | `lobby_start::map_variant() -> Option<MapId>`, beside the lobby's map; the pick's name and build in `map_variant::picked()` | `crates/extensions/game/src/{lobby_start.rs, map_variant.rs}` |
| A Live session | `NewSession.map_id`, which exists; `map` stays the base map's name | `crates/live/live-proto/src/session.rs` |
| A playlist entry | `map_variant: <MapId>` on a record of the hopper JSON. The retail record has no room for a hash, so the server keeps it in a table by hopper and record position, re-keyed by the patch's `from` as the other bytes are ([live-admin.md](../live/live-admin.md), "`PUT /v1/titles/{title}/hopper`"), published as `HopperVariantsSet` and served with the pins | `crates/live/live-server/src/admin.rs` |
| A game variant | `maps: [{base, variant, name}]` in the `.h2variant` bundle: map variants its author made it for. The `0x130` record is untouched | `variants::portable::Bundle` |

- **A playlist entry is a pair.** The record's level id must be the
  variant's base map, and the server checks it at publish, with the
  document published, not hidden and of the variant tier. Publishing
  an entry with a variant pins that `MapId` on its hopper, which is
  what lets a ranked session take it
  ([maps.md](maps.md) section 5). Two records of one hopper with the
  same level id and the same variant record must name the same map
  variant, so the host can tell which record matchmaking drew from the
  map and variant it was handed (**S**: the drawn record's index is
  not read at any named point).
- **A game variant only suggests.** Picking a game variant never
  changes the lobby's map or map variant. When the lobby's map has a
  suggested variant on this machine (`map_variant::suggest`: the
  suggestion's base is the installed map's hash and its `variant` the
  saved document's id), the Map Variant picker opens on it, with Original
  still first and the rest by name, and its line ends "Made for
  <game variant>."; a held pick wins, and a suggested variant that fails
  the store's checks is not opened on. The suggestions are written by
  Studio's variant tool (**Suggest map variants**, `variant.maps` and
  `variant.export`) and by `game_variants::store::Store::set_maps` for a
  game variant kept in the game's store, from the map variants on this
  machine (`forge::files::variants::suggestable`); the retail editor
  saves through the save storage, whose variants port with none. What
  a game variant *requires* of a map is its script's labels, which the
  start gate enforces.

### The lobby

Game Setup has **Map Variant...** under **Change Map...** in the
CUSTOM GAMES and FORGE modes (`lobby_mode::arrange`); in FORGE it picks
the map variant the session opens.

1. **Change Map...** picks the map
   ([map-select.md](map-select.md)). A map with saved variants opens
   its page, the map itself and then each variant
   ([map-select.md](map-select.md#a-map-with-variants)). A new map
   picked without a variant clears the variant.
2. **Map Variant...** opens a picker dialog on top of Game Setup
   (`kit.picker.map-variant`):
   **Original**, then this map's variants by name. The description
   gives the author, the counts by kind and the game types it has
   goals for (`forge_screens::variants::detail` writes the author and the
   counts by kind; the game types are not in it yet). A variant that cannot
   be used is listed with its reason as its description and cannot be chosen.
3. The lobby's panel shows the variant's name on the line under the
   map's (`lobby_mode::panel_lines`).
4. In FORGE the pick is what the session opens: Original is a new
   variant of the map.
5. **Start Game** materialises first when the package is not cached
   and is refused, with the reason as the lobby's status line
   (`lobby_status.rs`), when any check of "Rules and limits" fails for
   the host or a member. `map_variant::pick` loads the variant with the
   installed map's hash (`variants::load`), starts the build once on its
   own thread, and holds the start state
   (`lobby_mode::start_refused`) while it builds (the line says
   `Building <name>...`) or is refused (the table's sentence). A pick is
   cleared by a change of the lobby's map (`map_variant::sync`, read
   where the lobby's state is read), by Original and by leaving the lobby.
   In FORGE mode with no pick, `map_variant::follow_mode` (called by
   `lobby_mode::variant_hold`) picks the map's empty draft with the
   kit (`pick_kit`): the package carries the `forge_kit` spare rows, and
   `kit_start` names it for the load, where `forge_map::open` lets a
   session open on it and reads its spare rows into the session's kit
   ([forge-editing.md](forge-editing.md), "The object menu").
   Without a builder in the game (`game::map_variant::install`) a pick
   is `variant_build`. `variant_member` is `Refusal::member`; V3 sends it.

The row is absent when the `forge` feature is off. Only the party
leader picks, as for the map.

### Materialising for a match

Every machine builds the map from its own disc; no built byte travels
([forge-design.md](forge-design.md), "Materialising").

- **The host** builds when the leader picks, off the game thread, into
  `<data>/maps/forge-<first 16 hex of the id>/`, and reuses a package
  whose receipt matches: its manifest's id is the pick's and its files
  are there (`boot/src/forge_maps.rs`, `DiscMaps`, over the `Maps` trait
  of `map_variant.rs`).
- **A member** learns the pick from the session's `map_id`.
  `live_hle::maps::MapGate` treats a format 2 id as a package to make
  ready: get the document, check it, build it, compare with `expect`.
  A member is ready when its package resolves.
- **Getting the document.** From this machine's store by id; else from
  Live's content store by hash. A host in a Live lobby whose pick is
  not on Live uploads the document unlisted when it picks (a blob with
  no `ForgePublished` event, so it is in no list). In a System Link
  game without Live a member must have the file; a member who does not
  is named in the host's refusal. The Forge session channel of slice
  F5 carries a document to a joiner and replaces that refusal.
- **At load.** The lobby's map stays the base's level id. The session
  selects the package for that map's path
  (`content::Library::resolve_package`, which Forge packages need an
  explicit selection for), so the game loads the built file where it
  would load the disc's. The selection ends with the session.
- **A dedicated host** does the same from its host data, and refuses a
  session whose variant it cannot build before it accepts players
  ([dedicated-host.md](../live/dedicated-host.md)).

### Live

- **Store.** The document is a content-store blob by SHA-256, parsed by
  `forge-doc`'s validator, announced by `ForgePublished`
  ([forge-design.md](forge-design.md), "Saving and sharing on Live").
  The event's projection keeps the tier, the base's map name and hash,
  the name, the author and the counts by kind, so
  `GET /v1/map-variants?base=<sha256>` lists a map's variants and
  `GET /v1/map-variants/{id}` gives one. A preview picture is a second
  blob under the map packages' rule (512 KB, PNG or JPEG).
- **Sharing.** The Live site has a page a map with its variants and a
  page a variant; **Download** puts it in the signed-in player's list,
  which the game fetches into `live/` at sign-in as it does game
  variants. In the game, **Upload to Xbox Live** is a row of a
  variant's actions ([forge-map-variants-ui.md](forge-map-variants-ui.md)).
- **Limits.** A variant is refused over 1 MiB and over the document's
  op cap; the account quota and moderation are the map packages'
  ([live-admin.md](../live/live-admin.md), section 7). The name and
  description are moderated as gamertags are.
- **Both homes.** Routes, the event and the checks are in the local
  server and the Worker, and the parity suite covers them
  ([live-worker.md](../live/live-worker.md)).
