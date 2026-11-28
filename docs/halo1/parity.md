# Behaviour parity on converted maps

**Built.** `h2 parity` reads Halo 1's Xbox
executable and source cache from the owner's disc. It takes observations
through the agent socket on a running Halo 2 serve. Each comparison prints a
tab-separated check name, Halo 1 value, observed value and PASS or FAIL.
Missing data, a refused give, an unresolved state, a missed target or a
missing bot fails the case. The command returns non-zero on failure.

## Coordinator run

Use the integrated desktop `h2` on PATH, its matching prebuilt game image,
and the data root `h2 import halo1` wrote to. The parity runner opens each
level from the import's tag store (`<data>/tags`) by stem, so a plain import
is enough: there are no `.map` files to produce. Run from the repository
root. Replace the image and data paths with the integration build's
directories.

```sh
H1_ISO="$PWD/disc/halo1.iso" H2_DISC="$PWD/disc/halo2.iso" \
H2_IMAGE="/path/to/prebuilt-image" H2_DATA="/path/to/installed-data" \
scripts/h1-parity.sh
```

The script runs five headless serves in sequence. Each Blood Gulch sandbox
gets one two-seat Slayer round for control timing and one solo run for
weapon, animation and hit probes. This keeps bot input and incoming fire out
of the weapon and hit measurements. a30 uses a normal-difficulty campaign
serve with the Halo 1 assets. The script prints its log directory and returns
failure if any pass fails. It uses its own sockets, settings and snapshot
directory, stops each child on exit, and disables automatic disc imports. It
stops with a named error when a stored level it needs is missing.

| Case | Stored level (stem) | Serve environment |
|---|---|---|
| `bloodgulch-h1` | `h1_bloodgulch_h1` | `H2_LEVEL=h1_bloodgulch H2_HALO1_SANDBOX=h1` |
| `bloodgulch-h2` | `h1_bloodgulch_h2` | `H2_LEVEL=h1_bloodgulch H2_HALO1_SANDBOX=h2` |
| `a30` | `halo1_campaign_a30_h1` | `H2_LEVEL=a30 H2_HALO1_SANDBOX=h1` |

For example, these are the serve and runner commands for a single solo pass.
Use different terminals with the same environment.

```sh
H2_OPT=2 H2_LEVEL=h1_bloodgulch H2_HALO1_SANDBOX=h1 \
h2 serve "$H2_DISC" --out "$H2_IMAGE" --data "$H2_DATA" --settings "$SETTINGS" \
  --no-halo1 --no-mappack --socket "$SOCKET"
```

```sh
h2 parity "$H1_ISO" "$H2_DATA" bloodgulch-h1 "$SOCKET" --objects-only
```

The runner's second argument is the data root. It opens the case's stem
(`parity::stem`) from the store, checking the level's marker against the
stored file (`parity::open_level`). For the second sandbox use `h2` and
`bloodgulch-h2`. For round timing the script supplies `H2_SIM` with a
two-seat match file and uses `--round-only`. Without a phase switch the
runner performs all checks applicable to the map on the supplied socket.
Before changing state it checks the cache name, cache type, live weapon
datums and names, and the multiplayer player's biped.

## Boot sweep

`scripts/h1-boot-sweep.sh` boots every Halo 1 level in the store, one
headless serve on the fast tier each, and prints one tab-separated row per
level:

| Column | Source |
|---|---|
| `stem` | the store's `*.converted` marker (`halo1_campaign_<level>_<h1\|h2>` or `h1_<map>_<h1\|h2>`) |
| `state` | `ok`; `boot` when `session.wait` for `gameplay` at `SWEEP_TICKS` map ticks (600) failed; `panic` when the serve log holds a panic or `unimplemented!` line |
| `residual` | `render.residual` |
| `ledger_loops` | `sim.ledger` `loops` |
| `arms` | `sim.arms` |
| `first_panic` | the first panic line of the serve log |

```sh
H2_DISC="$PWD/disc/halo2.iso" H2_IMAGE="/path/to/prebuilt-image" \
H2_DATA="/path/to/installed-data" scripts/h1-boot-sweep.sh [stem...]
```

A campaign stem boots with `H2_LEVEL=<level>` and `H2_HALO1_ASSETS`
`halo1` (`_h1`) or `halo2` (`_h2`); a multiplayer stem with
`H2_LEVEL=<map>` and `H2_HALO1_SANDBOX`. The script exits non-zero if any
row is not `ok`.

## What is measured

| Check | Halo 1 reference | Observation and decision |
|---|---|---|
| Trigger and cadence | The `weap` trigger array at `+0x4fc` supplies flags, rates, acceleration time, charge time and rounds per shot. The XBE at `0xbdd69` supplies the press-latch mask. The firing calculation at `0xbdd14` names the tick-frequency float. | Give the named weapon and use the local pad. Compare the shot ticks for a sustained pull and for separate presses. For semi-auto weapons also issue rapid alternating press/release inputs. Compare consecutive automatic shots to the source firing rate during spin-up and at speed. A gun with no trigger must emit zero shots. |
| Shot count and spread | Trigger `+0x6e` gives pellets per shot, `+0x70` distribution angle, and `+0x78..0x80` minimum error and cone bounds. `+0x38` gives error acceleration time. | Count spawned projectiles in each barrel event. Read the applied cone from the fire routine, the live barrel fraction at `+0x1c` and the final direction of each launch. Check the interpolated cone, the direction envelope, that multiple spread samples actually vary from the unperturbed direction, and the fully bloomed cone under sustained automatic fire. |
| Ammo, reload and heat | `weap +0x4f0` magazines give capacity, reload duration and rounds per reload. The trigger gives cost per shot and heat per round. The weapon gives cooling per second. | Compare the fresh magazine, rounds after separate presses and first-shot heat increase. Start a reload from known loaded and reserve counts with the normal reload button. Count from the first non-idle magazine state to the first loaded-count increase and check the filled magazine. |
| Animation selection | Follow the `bipd` animation reference to `antr`. Read the unit, class and weapon-type slot indices, then read each referenced clip's name and frame count. A borrowed slot retains the clip it actually names. | Ask the live biped's native selector for stand, move, crouch, jump, aim, fire, reload, melee, throw and death requests where the source assigned a slot. Compare graph name, selected clip, index and frames. The printed result includes the engine's selected path. Both games' labels are expressed in Halo 2's naming vocabulary. The state request exercises graph lookup; playback, blending and the choice to enter a state are separate behaviours. |
| Collision and damage | Follow the biped's `coll` and model references. Use the default permutation of each node and a face of each distinct surface material. Read its edge ring, plane normal, region, node name and material type. The XBE body and shield readers (`0x94f3f`, `0x94a37`) name the collision-material multipliers. `0x956ab` and `0x9505c` locate the damage effect's per-material coefficients. | Launch an assault-rifle projectile towards the face centroid in its current node pose. Check the actual victim, region, node and material. Compare shielded and unshielded impact vitality with the source rifle projectile's impact damage range, material multiplier and target vitality. Restore the same pre-hit snapshot for each trial. |
| Round control | Halo 1 has no multiplayer bots. The reference is the participant's first applied control packet in the same converted round. | Compare the other seat's first movement/fire tick against seat zero's first control tick. An early action or missing participant fails. This is a relative ordering check, with no fixed countdown duration. |

Blood Gulch's Halo 2 sandbox uses the project's declared weapon role mapping.
Its collision reference uses the projectile actually launched by the mapped
rifle. Differences from Halo 1 still appear as failures. a30 uses a live
player or actor of the source biped type when present, and instantiates that
exact biped for a controlled trial when the type is absent at the checkpoint.
Weapons or actors the engine refuses are reported by name. The four probes
cover simulation behaviour. Pickup-prompt display requires a HUD check.

Blood Gulch’s source shotgun has a clear press-latch bit and a fixed repeat rate, so the held-trigger check expects shots spaced by that rate. A fast repeating shotgun fails on cadence. The source assault rifle's spread grows over its error acceleration time.

## Tolerances

- Automatic fire and reload transitions are measured at tick boundaries.
  A fractional interval can be reported one tick either side of the ideal
  duration. A semi-auto pressure sweep has a new press every two ticks. The
  observed interval must lie from the floor of the source interval through
  its ceiling plus one tick for the next press.
- First-shot heat is sampled at the boundary after the shot. Between zero
  and one source tick of cooling can have occurred in that sample.
- Damage uses the disc's own random damage bounds. The comparison applies
  the bounds in single precision to the pre-impact vitality. Pellet spread
  compares the measured unit-vector cosine to the source angular envelope
  in single precision. Clip identity, frame counts, ammo, pellet counts,
  collision identity and round ordering have no tolerance.

## Integration tests

```sh
cargo test -p agent probe
cargo test -p h1objects --lib parity
cargo test -p cli --bin h2 parity
H1_ISO="$PWD/disc/halo1.iso" H2_DISC="$PWD/disc/halo2.iso" \
  cargo test -p h1objects --test parity_disc -- --ignored --nocapture
```
