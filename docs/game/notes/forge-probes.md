# Forge probe measurements

Evidence for [Forge design](../forge-design.md) and [Forge features](../forge-features.md).

## Measurement procedure

Status **to be measured**. Procedure prepared 2026-11-21. With each run,
record the date, the tested commit, image and implementation switches,
local cache identities, network topology, and tick windows. Keep raw
agent output, tag names and local fixtures outside Git. This note takes
aggregate counts, byte gaps, timings and verdicts.

Use the [agent methods](../../runtime/agent-api.md#forge-probes) on the
coordinator's own disc runs. For the two-peer experiments, use a
[playerless System Link host](../../live/dedicated-host.md#playerless-lobby)
and a client, with separate data roots and agent endpoints. The commands
below assume `HOST_SOCKET` and `CLIENT_SOCKET` name those endpoints.
Set `MAP` to a multiplayer map the two peers share.

```sh
host_agent() { h2 agent --socket "$HOST_SOCKET" --timeout 100 "$@"; }
client_agent() { h2 agent --socket "$CLIENT_SOCKET" --timeout 100 "$@"; }
host_agent session.methods
client_agent session.methods
```

### Capacity

For each installed multiplayer map, start a stock local match or a
distributed control match. Use the same variant and record the host
sample after the same number of gameplay ticks each time. Take another
sample once weapons and vehicles have appeared.

```sh
host_agent wait '{"phase":"gameplay","map_ticks":90}' 90000
host_agent forge.capacity '{}'
```

Record both arena cursors, their byte gap, live and maximum object slots,
each block's count and known limit, and each marked tail count. Include
the tightest map. The byte gap measures the allocator at that tick.
Measure loading and play on kit builds separately to determine how
much of the gap the cache can give up.

### Host object replication by group

Open the distributed control with a stock variant. Have the client
join via System Link. When `host.lobby.state` reports `ready:true`,
start the match, wait for gameplay on both agents and confirm
`mp_host` and `mp_client` using `sim.session`.

```sh
host_agent host.lobby.open "$(jq -nc --arg map "$MAP" '{map:$map,variant:"slayer",max_players:2,live:false}')"
host_agent host.lobby.state '{}'
host_agent host.lobby.start '{}'
host_agent sim.session '{}'
client_agent sim.session '{}'
```

For each object group the API accepts, use an available tag from that
group and a clear position in a loaded BSP. The host's `edit.tags`
lists tags by group. Tag and position are controlled inputs so the
observation can be repeated. Record unavailable tags and engine
refusals separately from successful creations. The example uses
`GROUP=bloc`, `TAG` set to a tag name or datum, and `POS` set to a JSON
array of world coordinates obtained through the agent.

```sh
host_agent sim.objects "$(jq -nc --arg group "$GROUP" '{kinds:[$group]}')"
client_agent sim.objects "$(jq -nc --arg group "$GROUP" '{kinds:[$group]}')"
host_agent forge.probe "$(jq -nc --arg group "$GROUP" --arg tag "$TAG" --argjson pos "$POS" '{what:"replication",group:$group,tag:$tag,pos:$pos}')"
host_agent sim.objects "$(jq -nc --arg group "$GROUP" '{kinds:[$group]}')"
client_agent sim.objects "$(jq -nc --arg group "$GROUP" '{kinds:[$group]}')"
client_agent wait '{"ticks":30}' 10000
client_agent sim.objects "$(jq -nc --arg group "$GROUP" '{kinds:[$group]}')"
```

Check the host's returned handle first. On the client look for that
handle and validate group, definition datum and pose. If the handle
differs, use the before/after set, definition datum and position to
establish an unambiguous match. Take the first post-spawn snapshots
immediately for short-lived objects. Record first sighting delay and
whether the host object persisted through the observation window. A
missing client object counts as a negative result only if the host
held the object throughout the window and the client was in the same
loaded BSP. Start a fresh match for each ambiguous or destructive
trial.

### Synchronous protocol on a multiplayer map

After the distributed control, close the host lobby. Open the same map
with `synchronous:true` and omit the variant. Join and start through
the same steps as the control. Record whether open, discovery, join,
readiness, start and level load each complete. Preserve the first
refusal or timeout with both peers' API replies.

```sh
host_agent host.lobby.close '{}'
host_agent host.lobby.open "$(jq -nc --arg map "$MAP" '{map:$map,synchronous:true,max_players:2,live:false}')"
host_agent forge.probe '{"what":"sync"}'
host_agent host.lobby.state '{}'
host_agent host.lobby.start '{}'
host_agent wait '{"phase":"gameplay","map_ticks":30}' 90000
client_agent wait '{"phase":"gameplay","map_ticks":30}' 90000
host_agent forge.probe '{"what":"sync","max":1}'
client_agent forge.probe '{"what":"sync","max":1}'
```

Expect protocol `synchronous`, roles `coop_host` and `coop_guest`,
two peers, and scenario type 1 on both. Record each active engine
index. An index of zero tests a variant-free multiplayer load. Evidence
for a running game engine also needs an active engine and observable
game rules. Record whether the client acquires a unit and whether its
input changes the host's view. After both are ticking, choose
`FROM_TICK` at or after the higher observed tick. Let at least 600
more ticks elapse and save both windows under the retained output
directory `PROBE_OUTPUT`.

```sh
host_agent wait '{"ticks":600}' 60000
params=$(jq -nc --argjson from "$FROM_TICK" '{what:"sync",from_tick:$from,max:600}')
host_agent forge.probe "$params" >"$PROBE_OUTPUT/sync-host.json"
client_agent forge.probe "$params" >"$PROBE_OUTPUT/sync-client.json"
```

Compare `hashes.rows` by tick over the common window. Record the first
differing gameplay digest, any missing ticks, and both desync latches.
A comparable window needs 600 consecutive shared ticks with equal
gameplay digests and clear latches. An empty series, failed load or
lack of an active engine leaves that part of the experiment open.
Observe both idle and during ordinary player actions through the
agent. Record arena digests separately if that optional collection
is enabled.

### AI in multiplayer

Take a stock multiplayer baseline on the distributed host and in the
synchronous run if it starts. Compare it with a campaign run that
already has active actors to validate the reader. When a local fixture
with a valid character, squad, starting location and pathfinding is
available, load it as multiplayer. Sample before and after its authored
squad placement trigger on both peers. Use matching ticks for the stock
and fixture runs. Confirm roster or movement with `world.ai`. A
zero-actor stock map provides a baseline. A valid authored squad is
needed to test whether that mode runs AI.

```sh
host_agent forge.probe '{"what":"ai"}'
host_agent world.ai '{}'
client_agent forge.probe '{"what":"ai"}'
```

Record `enabled`, `active`, actor counts, mode and whether the
authored trigger fired. The actor fixture and any game content stay on
the measurement machine.

### Inert kit entries

With a local empty-document kit build available, load the unmodified
base and the kit map with the same match settings, seed where
controlled, and input. Start with an isolated spare tail for each
block family, then test the combined kit. Take samples after load,
first spawn and equipment respawn. Match the tick windows and use
`sim.objects` and `world.ai` on each run as well.

```sh
host_agent forge.capacity '{}'
host_agent forge.probe '{"what":"kit"}'
host_agent sim.objects '{}'
host_agent world.ai '{}'
```

Confirm the authored spare counts and marked tails against the baseline.
For device groups and squads, compare with the kit build's reservation
counts. Check that each tail leaves its readers' behavior intact:
ordinary placement and naming, player starts, objectives and teleporter
selection, equipment spawn and respawn, device activation, and the
actor roster. Include a positive control with one functional entry in
place of a spare to confirm the exercised reader can observe it.
Record load errors, extra objects or actors, changed behavior, and
untested readers by block. An equal aggregate census can corroborate
the behavior checks but cannot establish inertness by itself.
