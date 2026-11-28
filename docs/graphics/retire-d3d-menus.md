# Retiring the guest D3D path: the menus (R7)

Status: Partly built. The menu frame's widgets, text (substituted in Rust, button glyphs included), fade, bars, loading screen and blur draw from the scene's overlay where they read; every UI draw that does not is counted kept by kind and cause. Text and draw classes with a named gap keep guest packets. Model widgets are the overlay's, with no guest draw ([native-menus-overlay.md](native-menus-overlay.md#model-views)).
Code: `crates/rewrites/render-bridge/src/native_hud.rs`, `native_hud/ui_overlay.rs`, `native_hud/ui.rs`, `native_hud/kept.rs` (`UiKind`, `begin_ui`, `ui`), `native_hud/ui_overlay/text.rs` (`substitute`), `ui_overlay::Gap`, `crates/extensions/game/src/hud_overlay.rs` (`Widget::plan`, `corner_order`), `native_hud_glue.rs` (`Glue::offer`). Tests: `render_bridge::native_hud::kept::tests`, `ui_overlay::tests`, `ui_overlay::bitmaps::tests::a_meter_reads_as_one_rectangle_of_the_meter_colour`, `ui_overlay::bitmaps::tests::a_scrolling_bitmap_reads_as_its_four_wrapped_quads`, `ui_overlay::bitmaps::tests::an_image_set_from_elsewhere_is_named_by_its_bitmap_tag`, `game::hud_overlay::tests`, `scene_extract::ui::tests::the_frame_draws_in_the_guests_order` (the UI order test), `scene_extract::ui::tests::a_labels_button_glyph_is_a_character_to_fonts_that_lay_buttons_out`, `ui_overlay::text::tests`, `ui_overlay::tests::a_widget_gap_is_kept_under_its_own_name`.
Switch: `impl.render.native` (on), listed in [switches.md](../runtime/switches.md).

Slice R7 of [retire-d3d.md](retire-d3d.md#slices). The menus share the overlay stage (`frame_scene::Overlay`, `FrameScene::overlay`) with the HUD of slice R6; R7 adds no stage of its own. The rule: a UI draw draws from the overlay or is counted `scene_kept_ui_<kind>_<cause>` at the first producer that refuses it. Nothing is dropped.

## What draws from the overlay

| Draw | Taken at | Page |
|---|---|---|
| Containers, plain bitmap widgets, supported text, lists and buttons | the sorted list `0x002353a5`, one pass per run | [native-menus-overlay.md](native-menus-overlay.md#the-menus-on-the-overlay) |
| Text with named sequences and button glyphs: the substitution pass `0x0022d2ee` in Rust | the sorted list, as text | [native-menus-overlay.md](native-menus-overlay.md#what-the-list-reads) |
| Meters: one rectangle of the UI globals' meter colour | the sorted list, as a bitmap widget | [native-menus-overlay.md](native-menus-overlay.md#meters) |
| Scrolling bitmaps: up to four wrapped quads | the sorted list, as a bitmap widget | [native-menus-overlay.md](native-menus-overlay.md#scrolling-bitmaps) |
| Bitmaps whose image is set at `+0x8c`: named by the bitmap tag that holds it | the sorted list, as a bitmap widget | [native-menus-overlay.md](native-menus-overlay.md#bitmap-widgets) |
| Shader bitmaps | the shader quad `0x0022a664` | [native-hud-overlay.md](native-hud-overlay.md#bitmap-widgets) |
| Fade, all six modes | `0x00040d86` | [native-menus-overlay.md](native-menus-overlay.md#the-fade-the-bars-and-the-loading-screen) |
| Bars: fans, quads, polygons and strips | the uploader `0x0004b252` from `0x00052ce7` | this page, "Bars" |
| Loading background, sweep and spinner | `0x0002ae60`, `0x0002a1f4` | [native-ui-producers.md](native-ui-producers.md#native-ui) |
| Blur under a dialog | `0x00235756` | [native-menus-overlay.md](native-menus-overlay.md#the-blur-under-a-dialog) |
| Model widgets | the sorted call: the draw is skipped; an active one is a `ModelView` item | [native-menus-overlay.md](native-menus-overlay.md#model-views) |

The menu's 3D view (the city flyover, most of the menu frame's draws) is a game view: its layers draw through the world, object and late stages of slices R1 and R5. The UI scope (`Host::enter_ui`) drains the device's admission boundary first, so the view's work is the scene's and the menu's draws follow it.

### Bars

`hud_overlay::corner_order` puts a 4-vertex draw's vertices in the overlay's corner order, so the overlay's two triangles (corners 0, 1, 2 and 0, 2, 3) are the draw's own, with the same windings:

| Primitive | Corners | Diagonal |
|---|---|---|
| fan, quad list, polygon | 0, 1, 2, 3 | 0 to 2 |
| strip (triangles 0, 1, 2 and 2, 1, 3) | 1, 3, 2, 0 | 1 to 2, the strip's |
| strip whose 0 and 3 are not on either side of 1 to 2 | refused, `strip_shape` | the triangles overlap and would blend in the other order |
| any other | refused, `primitive` | |

## What is kept

`scene_kept_ui_<kind>_<cause>` (`render.scene`) counts each menu-frame draw that does not draw from the overlay. It shares the HUD's register, `render_bridge::native_hud::kept` ([native-hud-overlay.md](native-hud-overlay.md), `scene_kept_overlay_*`): a screen draw is offered (`begin_ui`, by `Glue::offer`), the capture names the first cause that refuses it (`Widget::plan`, `kept::refuse`), and it is counted when the offer ends undrawn, or `packet` when the native UI packet path records it (`kept::carried`, the carried grade R8 removes).

Producers that refuse before any offer count their own (`kept::ui` for the `Ui` counters that are kept draws, `kept::count`). Totals since start, independent of the frame-stats window. In the HUD scope the HUD's producers name the item, so these kinds are the menu frame's.

| Kind | Draw | Causes |
|---|---|---|
| `widget` | a drawn widget of a sorted list, once per widget whatever it draws | `bitmap_state`, `unreadable`, `unlisted`, `pass` (per widget of a run not placed); a text, class or model gap by its own name: `text_*`, `list_*`, `button_*`, `art_*`, `draw_class`, `draw_runtime_class`, `draw_custom` ([native-menus-overlay.md](native-menus-overlay.md#what-stays-with-the-guest)) |
| `list` | a whole sorted list | `frame` (viewport not the full canvas, or rounding not to nearest), `mismatched`, `pass` (the UI scope's order did not hold) |
| `fade`, `bar`, `loading`, `quad` | a screen draw (`quad`: a generic widget fan or screen quad) | the capture's: `target`, `clip_rects`, `depth`, `fill`, `query`, `colour_write`, `shader`, `combiner`, `sampler`, `texture`, `vertices`, `strip_shape`, `primitive`, `culled`, `clip`, `overlay`, `device`; the producer's: `order`, `push`, `scope` (no overlay scope names it), `reservation`; `packet` |
| `menu_shader` | a menu shader bitmap's quad | as `fade` |
| `outline` | a widget outline | `entry`, `inputs`, `reservation`, `packet` |
| `blur` | a blur under a dialog | `replayed`, `pass`, `arguments`, `steps`, `combiner`, `targets` |

## Per-frame work

At each sorted list the guest's widgets are read and listed again every frame (`claim_list`), so focus, colour and animation changes reach the pass. The listed items are not copied on the way to the GPU:

| Step | What it holds |
|---|---|
| `ui_overlay::run_part` | moves the run's planned items out of its claim; a run is placed once |
| `menu_item`, `Host::ui_pass`, `place_menu` | one `Arc<Overlay>` shared by the pass and the scene's overlay |
| `OverlayPass::encode` | plans the shared items in place; only a pass with a UI fit copies them to move them |

## Rules and limits

- A kept draw keeps its guest order: the producer returns to the guest at the same place in the list.
- Only `widget` and `list` causes are per sorted list; the others are per draw.
- The cause is the first refusal on the path. A draw the packet path records after an overlay refusal counts once, as `packet`.
- A UI item's guest draws are not the HUD's `untraced` ones: a kept menu draw owes the HUD no draw.
- A widget gap counts its draw's `Ui` counter (`native_ui_overlay_menu_refused_text`, `_draw`) and its own cause, never the draw's bare name too.

## Not verified

- The exit captures: main menu, settings, pregame lobby and loading match the guest's draws exactly at 1x, and the `scene_kept_ui_*` rows on each.
- Strip bars: which menus draw them, and that their captures match.
- Substituted labels and button glyphs: that their glyph quads match the guest's on the main menu and the pregame lobby.
- The menu frame rate at 2560x1440 and what the per-frame cuts save. Perf numbers come from `h2 lab` ([menu-budget.md](../perf/notes/menu-budget.md) is the last menu measurement).

## Related

[retire-d3d.md](retire-d3d.md), [native-menus-overlay.md](native-menus-overlay.md), [native-ui-producers.md](native-ui-producers.md), [native-hud-overlay.md](native-hud-overlay.md), [native-renderer.md](native-renderer.md).
