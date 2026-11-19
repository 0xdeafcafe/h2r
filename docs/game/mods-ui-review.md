# Mods UI review

Reviewed branch base `d1fd1f2f`. The supplied disc previews cover Mods and Sample Pack at rows 0, 2 and 3, and Lockout Swords at row 0. I inspected all seven PNGs and their widget models. The level-select bracket, bright focus bar, Conduit text and footer read as Halo 2. The detail page's single action, blue status panel and picture also read as the pregame lobby. The supplied names fit, including the star on the last pack row. The checklist heading floats between its columns, and the detail hint sits on the lower bevel of the blue panel.

## Findings at the review base

| Severity | Screen | What the player encounters | Fix |
|---|---|---|---|
| High | Mods, cold open or retry | The guest list is allocated for one loading row. When four rows arrive, row 0 becomes a map but the other three have no selectable slots. A background refresh can also replace the map under the cursor or remove an open page's data. | Reserve room for the catalogue, expose rows when the list arrives and keep the populated catalogue stable through that visit. Give an explicit retry priority over the Live menu's refresh throttle. |
| High | Pack | X with no focus or a stale row falls through to Download All. An empty pack reads All Installed because every map in an empty collection passes the installed check. A missing pack displays Back on a row that does nothing. A full 40-map response hides the last map below Download All. | Require row 0 for bulk actions, show how to leave an unavailable pack, and reserve one row in addition to the map limit. |
| High | Detail, Mods and pack footers | Download All says A Details. Loading and installed rows advertise X Download. The page can still show Download after it finishes and Remove after removal because the footer widget copies its string on open. The adjacent A and X glyphs on the detail sample also look like a chord. | Give each focus and state its actual commands. Refresh the guest footer with the focused description. Keep removal on the detail page with its second-press confirmation. |
| Medium | Mods and pack | A failed download can still wear the new star. Its generic error tells the player to press A to retry, which opens details on the list. Pack progress can fall when a completed download leaves its denominator. | Show failures on rows, put recovery instructions on the button that retries, and count completed bytes in batch progress. |
| Medium | Detail and summaries | The ten fact label/value pairs share entire rectangles. Long credits and feature lists can consume the status space, and limits measured in characters don't account for wide letters. The light hint crosses the panel bevel and the checklist heading is indented 78 units from its first column. | Bound published text within the actual columns, give status its own clear line, and align the checklist heading with its cells. Name Change Map's Mods category in the installed hint. |
| Medium | Browser pictures | A missing slot image leaves the previous row's picture in the widget. The first cold row can show campaign art while it reads the list. | Use the lobby's unknown-map picture in the level-select frame. Restore it when the focused row has no bitmap, and update a resolved picture even if focus stayed put. |
| Medium | Preview coverage | All seven previews show new maps. The six list previews share the level-select sample picture and the detail preview shows the lobby's unknown-map art. These are template images, so they cannot establish the identity of a selected mod in Game Setup. | Add named preview states for loading, offline, empty, progress, failure, installed and removal. Check the selected package's identity in the integrated slot build. |
| Low | Pack names | In the sample the pack title is repeated on each map, leaving Beaver Creek close to the bar's right edge. | Omit an exact pack-name prefix in the pack list. Keep the full name on the detail page. |

## Integrated slot handling

Each installed mod that fits has its own slot in Change Map. Selecting
it uses the slot's package name, description and picture in Select Map,
Game Setup and the lobby. The disc or Map Pack sibling retains its
retail slot and art. The Mods browser and its detail page still use the
base filename for their previews. A missing base bitmap uses the
neutral lobby picture. [Map selection](map-select.md#separate-slots-for-mods) ·
[Preview sources](maps.md#a-mods-page)

The game check still needs two mods of the same base, one with an image
and one using the base fallback. Pick both, go back to the category
list with B, and switch to the retail sibling.

## Controller and state checks

| Focus | A | X | B |
|---|---|---|---|
| New or available map | Details | Download | Parent screen |
| Installed map on a list | Details | No action | Parent screen |
| Failed map on a list | Details and failure reason | Retry | Parent screen |
| Pack in Mods | Open pack | Download or retry eligible maps | Xbox Live menu |
| Download All | Start eligible downloads | Same as A | Mods |
| Map detail | Download, retry, or arm removal | Same as A | Parent row |
| Confirm removal | Remove | Remove | Cancel and return |
| Detail during download | No action | No action | Return while it downloads |
| Empty or offline catalogue | Refresh or retry | No action | Xbox Live menu |
| Empty pack or unavailable detail | No action | No action | Parent screen |

The default list and pack previews still include focus 0, 2 and 3. The
detail page opens on its single action. The named states also exercise
long names, partial installation, failure, signed-out access and removal.
The preview walkthrough starts on a pack, downloads one map, opens its
detail, then checks both parent focus positions on B. Y has no action.

## Verification boundary

This is an offline review. Navigation and download findings follow the Rust handlers and the list-class loader. B uses the retail stack. Actual focus restoration on B, bitmap replacement, transition timing and lobby presentation need the coordinator's integrated run. The existing agent API is appropriate for that check. No live session or API extension was needed to inspect the supplied artifacts.
