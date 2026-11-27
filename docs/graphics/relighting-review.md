# Reviewing one reconstructed room

Status: Partly built. The native capture accepts both baked states. Selecting a suitable fixture and judging the images require the owner's disc.
Code: `crates/apps/cli/src/lighting/split.rs`, `crates/extensions/game/src/frame_owner.rs`, `crates/platform/d3d/src/gfx_backend/guest_draw_pass/typed.rs`.
Switch: the [Lighting setting](../runtime/switches.md).

**Design.** Review Foundation, structure BSP 0, from the doorway of the fan
room facing the floor and an adjoining wall. The fan room is described in
[e3-lighting.md](e3-lighting.md). The proposed source is one stationary,
spherical, baked scenario light in that room. Its placement index and
eligibility are unverified. Read them locally. An empty eligible list, a
room lit solely by dynamic lights, or a failed fit rules the site out.

## Bake the selected lamp

1. Set `H2_DISC` to the owner's retail ISO. List the baked spheres in BSP 0.
   Eligible rows give `placed:N`, position and radius in world units, and
   the definition name. Refused rows give a reason. Use position to locate a
   candidate in the fan room. A retail view can be scouted with
   `render.view` and `render.capture` as below.

   ```sh
   h2 lighting split foundation --bsp 0 --disc "$H2_DISC" --list-lamps
   ```

2. Set `LAMP` to its printed ID, including `placed:`. Install the two states
   under the same host data root the game uses. The command prints the path
   to the private HTML report and refuses a nonempty destination. For a new
   bake, move the previous `relighting` directory to another private location.
   A STOP report has no usable runtime bundle. The page comparison should
   show the selected pool of light, a plausible on image, and detail left
   in the shadowed area. [Split model](dynamic-lighting.md#the-split-offline).

   ```sh
   h2 lighting split foundation --bsp 0 --disc "$H2_DISC" --light "${LAMP:?set the fan-room placement ID}" --install
   ```

## Capture the room

1. Launch the GPU game with the installed data root. Both reconstructed states
   are available for private draws in this run. A dedicated agent socket
   identifies it when another game is open. Use an existing image of this
   build with the same optimisation level used to produce it. The render
   thread queues the world stage so the scene recorder can retain the draw
   and bind a different atlas during each private replay.

   ```sh
   export H2_AGENT="${TMPDIR:?}/relight-review.sock"
   H2_LIGHTING=relight-on H2_RESOLUTION=1280x960 H2_RENDER_THREAD=1 \
     H2_FEATURES=impl.render.native=on,frame-scene.sink H2_TIER=fast \
     h2 play "$H2_DISC" --level foundation --fps 30
   ```

2. From a second shell on the same machine, select that socket. Wait for
   gameplay and check that `session.status.at.bsp` is zero. Set `POSE` to the
   doorway camera as JSON with `eye` (three world coordinates), `yaw` and
   `pitch` (radians), and `fov` (horizontal degrees). To scout from a lamp
   at `[x,y,z]`, start a few world units back along negative x, face
   positive x with yaw zero, and adjust the eye until the floor, fixture
   and a patch outside the direct pool are in view. Keep the resolved
   camera reply with the local review. The coordinates depend on the
   selected placement.

   ```sh
   # Use the exact H2_AGENT path from the launch shell if TMPDIR differs.
   export H2_AGENT="${TMPDIR:?}/relight-review.sock"
   h2 agent --timeout 185 session.wait '{"until":{"map":"foundation","phase":"gameplay","map_ticks":30},"timeout_ms":180000}'
   h2 agent session.status
   # LAMP_XYZ is the selected row's position, copied as a JSON array.
   # Supply POSE to reuse a surveyed doorway pose, or start near the lamp.
   if [ -z "${POSE:-}" ]; then
     POSE=$(jq -nc --argjson p "${LAMP_XYZ:?set the three coordinates from the placement row}" \
       '{eye:[($p[0]-2),$p[1],($p[2]-0.5)],yaw:0,pitch:-0.15,fov:75}')
   fi
   h2 agent render.view "$POSE"
   h2 agent wait '{"ticks":3}' 10000
   ```

3. Capture retail first to pin the sealed scene. Use the returned tick for
   both reconstructed states. This procedure uses `jq` and writes the three
   PNGs and comparisons to a fresh directory inside the private bake directory.
   All three replies must report the same tick, present and scene hash.
   A missing replacement or an unreplayable frame gives an RPC error.

   ```sh
   (
     set -eu
     review_root="${H2_DATA:-$HOME/Library/Application Support/h2recomp/data}"
     review_dir=$(mktemp -d "$review_root/relighting/review.XXXXXX")
     cd "$review_dir"
     h2 agent render.capture '{"lighting":"retail"}' > retail.json
     tick=$(jq -er '.tick' retail.json)
     for mode in relight-on relight-off; do
       h2 agent render.capture "$(jq -nc --arg m "$mode" --argjson t "$tick" '{lighting:$m,tick:$t}')" > "$mode.json"
     done
     jq -se 'all(.[]; .scene_hash != null) and (map([.tick,.present,.scene_hash]) | unique | length == 1)' \
       retail.json relight-on.json relight-off.json
     for mode in retail relight-on relight-off; do
       cp "$(jq -er .path "$mode.json")" "$mode.png"
     done
     for pair in 'retail relight-on' 'relight-on relight-off'; do
       a=${pair% *}
       b=${pair#* }
       params=$(jq -nc --arg a "$PWD/$a.png" --arg b "$PWD/$b.png" --arg diff "$PWD/$a-$b.png" '{a:$a,b:$b,diff:$diff}')
       h2 agent render.compare "$params" > "$a-$b.json"
     done
     printf 'Review %s\n' "$review_dir"
     jq -e '.max_channel > 0' relight-on-relight-off.json
   )
   ```

   Restore the ordinary camera when finished.

   ```sh
   h2 agent render.view '{"reset":true}'
   ```

## Art decision

Compare the on image with retail for colour, shadows, contrast and doorway
legibility. Compare on with off at the directly lit floor, the wall outside
its direct pool, and the fixture itself. An identical on/off pair cannot
demonstrate the switch. A frame-wide difference can hide a bad fit even
when the floor darkens. Inspect the entire image and the two difference
images. Repeat from a second pose if the doorway conceals either receiver.

The [split model](dynamic-lighting.md#reading-the-comparison) leaves the
source's bounce in the residual. The fixture's own material, guest light
passes and moving objects have separate lighting paths. Record what
changes at each surface. Accept per level only if the directly lit area
changes as intended and the remaining fill, fixture and silhouettes read
well. A rejected fit stays in private review.

## Not verified

- An eligible placement in the fan room, complete coverage of its pages,
  and a reviewable fit on the owner's copy.
- A successful retail/on/off capture from that room and the owner's art decision.
