# Sourced by scripts/readme-video.sh and scripts/readme-screenshots.sh: the
# scenes beyond the campaign levels, each a level to boot plus what to stage.
#
# The caller defines, before calling anything here:
#   agent <socket> <method> [params]   one agent API call
#   agent_wait <socket> <until-json>   session.wait (prints the answer)
# and the variables $h2, $root and $scratch.
#
# A scene is a row of the parallel arrays below. `prep` runs once gameplay
# has started, before the settle, and may restart the game (it then waits for
# the new game itself). `act` runs just before the frame or clip, so what it
# starts is happening on screen. `drive` is how the clip is moved: `pad`
# (the stick pan and walk), `bot` (player.bot plays the seat, no stick) or
# `none` (the act drives the player). `hud` is 1 to leave the HUD on.
#
# Phantom Clankers join only a hosted game, and a level boot is a local one
# (docs/runtime/agent-api-sim.md, "Phantom backend"); `h2 sim run` has no
# GPU. So the bot scenes are `player.bot`: a Clanker plays the local seat.
#
#   scene        level       prep        act         drive  hud  what happens
#   clankers     ascension   -           bot         bot    0    a heroic, wild Clanker plays Slayer
#   firefight    coagulation firefight   bot         bot    0    the Firefight probe's Elites vs a Clanker
#   forge        ascension   forge       -           pad    1    the Forge monitor flies, picks up and drops
#   siege        zanzibar    siege       bot         bot    1    Siege's attack phase, a Clanker attacking
#   superbounce  ascension   superbounce superbounce none   1    a drop onto a bounce strip, HUD and medals

scene_names=(clankers firefight forge siege superbounce)
scene_levels=(ascension coagulation ascension zanzibar ascension)
scene_preps=("" firefight forge siege superbounce)
scene_acts=(bot bot "" bot superbounce)
scene_drives=(bot bot pad bot none)
scene_huds=(0 0 1 1 1)

scene_index() {
  local i
  for ((i = 0; i < ${#scene_names[@]}; i++)); do
    [[ "${scene_names[i]}" == "$1" ]] && { echo "$i"; return 0; }
  done
  return 1
}

# scene_wait_game <socket>: wait for a fresh game's first second; sets
# $scene_map to its map name. The sleep lets a queued restart take hold, so
# the wait does not see the old game's ticks.
scene_wait_game() {
  sleep 3
  local ready
  ready="$(agent_wait "$1" '{"phase":"gameplay","map_ticks":30}')"
  scene_map="$(jq -er '.at.map' <<< "$ready")"
}

# scene_script <socket> <rules file>: restart the game on the same map with
# the script (rules.reload, mode restart: docs/runtime/agent-api-modes.md,
# "Rules scripts"). The blob is built here, so the script's `use` files
# resolve as `h2 rules build` resolves them.
scene_script() {
  local sock="$1" file="$2" rir="$scratch/scene.rir" params
  "$h2" rules build "$root/content/modes/rules/$file" -o "$rir" >/dev/null
  params="$(jq -cn --arg ir "$(base64 < "$rir" | tr -d '\n')" '{ir_b64:$ir, mode:"restart"}')"
  agent "$sock" rules.reload "$params" >/dev/null
  scene_wait_game "$sock"
}

prep_forge() {
  # D-pad Up toggles the local Forge monitor (docs/runtime/agent-api-modes.md,
  # "Local Forge session"): the camera leaves the player and flies.
  agent "$1" input.pad '{"steps":[{"buttons":["up"],"ticks":3},{"ticks":1}]}' >/dev/null
  sleep 2
  agent "$1" forge.status | jq -e '.session != null' >/dev/null || echo "forge: no session open" >&2
}

prep_siege() { scene_script "$1" siege.rules; }

prep_superbounce() { scene_script "$1" superbounce.rules; }

prep_firefight() {
  # The Firefight probe test map (docs/game/test-maps.md): a disc map with
  # one squad of two Elites placed on it, built into the data folder.
  local sock="$1" name="firefight-probe-coagulation" i state
  agent "$sock" maps.test.create \
    '{"recipe":"firefight-probe","base":"coagulation","replace":true}' >/dev/null
  for ((i = 0; i < 300; i++)); do
    state="$(agent "$sock" maps.test.list "$(jq -cn --arg n "$name" '{name:$n}')" |
      jq -r --arg n "$name" '[.maps[] | select(.name == $n) | .state][0] // "none"')"
    [[ "$state" == ready ]] && break
    [[ "$state" == failed ]] && { echo "firefight: the probe map failed to build" >&2; return 1; }
    sleep 2
  done
  [[ "$state" == ready ]] || { echo "firefight: the probe map was not ready" >&2; return 1; }
  agent "$sock" maps.test.load "$(jq -cn --arg n "$name" '{name:$n}')" >/dev/null
  scene_wait_game "$sock"
}

# act_bot <socket>: a Clanker plays player.0 (docs/runtime/agent-api-sim.md,
# "player.bot"); waits until its pad reaches the game.
act_bot() {
  local i
  agent "$1" player.bot '{"on":true,"difficulty":"heroic","style":"wild"}' >/dev/null
  for ((i = 0; i < 120; i++)); do
    agent "$1" player.bot | jq -e '.driving' >/dev/null && return 0
    sleep 1
  done
  echo "player.bot never drove the seat" >&2
  return 1
}

# act_superbounce <socket>: one drop trial onto the best strip, with the
# bounce primed (docs/runtime/agent-api-world.md, "superbounce"). The seat
# is teleported above the strip and falls, so the bounce shows with the HUD.
act_superbounce() {
  agent "$1" superbounce.verify \
    '{"kind":"drop","max_spots":1,"heights":[3.5],"across":[0],"save":false}' >/dev/null
}
