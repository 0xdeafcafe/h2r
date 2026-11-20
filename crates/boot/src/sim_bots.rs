//! `sim.player.*`'s bots (`agent::sim::bots::Bots`) over docs/game/bots.md's
//! `bots::phantom`.
//!
//! Every simulated player is a phantom peer (docs/game/bots.md §5.1): on
//! the books at once, it joins once a multiplayer game this console hosts
//! runs. A local controller seat is a real person's; `sim.player.add` with
//! `kind = "local"` is refused with an error saying so. A phantom's source:
//!
//! - a script is the `Remote` sim fills (`sim.player.add`'s steps, then
//!   `sim.player.pad`), read each tick with `state_at_tick` then
//!   `state_read`, as the agent's input tap reads `input.pad`'s;
//! - a recording is an input log's reads of its first pad, one per frame
//!   ([`bots::pad_source::Recorded`]);
//! - the brain is [`bots::Brain`].

use agent::sim::bots::{Bot, Bots, PadSource};
use agent_proto::sim::{BotBrain, BotMood, BotPersonality, ClankerView, Identity, PlayerKind};
use bots::pad_source::{self, Frame};
use input::backend::remote::Remote;
use input::gamepad::{Gamepad, State};
use input::host::HostPadState;

/// The bots sim calls: the process's phantoms.
pub struct SimBots {
    allow_add: bool,
}

impl SimBots {
    pub fn new(allow_add: bool) -> Self {
        SimBots { allow_add }
    }
}

/// A script source: the pad `Remote` holds now, timed in bot frames.
struct ScriptPad(Remote);

impl pad_source::PadSource for ScriptPad {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        let g = self
            .0
            .state_at_tick(std::time::Instant::now(), Some(f.frame));
        self.0.state_read();
        HostPadState::from_gamepad(g)
    }
}

/// An input log's pads: the successful reads of the first handle it read.
pub fn recording_pads(text: &str) -> Result<Vec<HostPadState>, String> {
    let log = xapi::tap::InputLog::parse(text)?;
    let Some(first) = log.reads.first().map(|r| r.handle) else {
        return Err("an empty input log".into());
    };
    Ok(log
        .reads
        .iter()
        .filter(|r| r.handle == first && r.status == 0)
        .map(|r| {
            let pad: &[u8; Gamepad::SIZE] = r.packet[State::SIZE - Gamepad::SIZE..]
                .try_into()
                .expect("a packet holds a gamepad");
            HostPadState::from_gamepad(Gamepad::from_bytes(pad))
        })
        .collect())
}

fn source(s: PadSource, seed: u64) -> Result<Box<dyn pad_source::PadSource>, String> {
    Ok(match s {
        PadSource::Script(r) => Box::new(ScriptPad(r)),
        PadSource::Recording { name, path } => {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("recording {name} ({}): {e}", path.display()))?;
            let pads = recording_pads(&text).map_err(|e| format!("recording {name}: {e}"))?;
            Box::new(pad_source::Recorded::new(pads))
        }
        PadSource::Brain => Box::new(bots::Brain::new(bots::mind::Skill::default(), seed)),
    })
}

/// An xuid as the server gives it: hex, `0x` optional.
fn xuid(s: &str) -> Result<u64, String> {
    let t = s.trim();
    u64::from_str_radix(t.strip_prefix("0x").unwrap_or(t), 16)
        .map_err(|_| format!("xuid {s:?}: hex, as the local embedded server issues it"))
}

fn brain_view(view: bots::BrainView) -> BotBrain {
    use bots::arms::Role;
    use bots::mind::Skill;

    let skill = match view.skill {
        Skill::Easy => "easy",
        Skill::Normal => "normal",
        Skill::Heroic => "heroic",
        Skill::Legendary => "legendary",
    };
    let p = view.personality;
    let favorite_weapon = match p.favorite_weapon {
        Role::Unknown => "unknown",
        Role::Blade => "blade",
        Role::Scatter => "scatter",
        Role::Spray => "spray",
        Role::Precise => "precise",
        Role::Sniper => "sniper",
        Role::Blast => "blast",
    };
    let m = view.mood;
    BotBrain {
        skill: skill.into(),
        personality: BotPersonality {
            aggression: p.aggression,
            caution: p.caution,
            teamwork: p.teamwork,
            objective_focus: p.objective_focus,
            composure: p.composure,
            patience: p.patience,
            showboating: p.showboating,
            weapon_loyalty: p.weapon_loyalty,
            chattiness: p.chattiness,
            favorite_weapon: favorite_weapon.into(),
        },
        mood: BotMood {
            confidence: m.confidence,
            tilt: m.tilt,
            fear: m.fear,
            streak: m.streak,
            revenge_target: m.revenge_target.map(|id| format!("player.{id}")),
        },
    }
}

impl Bots for SimBots {
    fn add_phantom(
        &mut self,
        identity: &Identity,
        team: Option<i32>,
        src: PadSource,
    ) -> Result<u32, String> {
        if !self.allow_add {
            return Err("light host bots are off; start with --host-bots".into());
        }
        let id = bots::phantom::Identity {
            xuid: xuid(&identity.xuid)?,
            gamertag: identity.gamertag.clone(),
        };
        let team = team
            .map(|t| u8::try_from(t).map_err(|_| format!("team {t}")))
            .transpose()?;
        let src = source(src, id.xuid)?;
        bots::phantom::add(id, team, src)
            .map(|h| h.0)
            .map_err(|e| e.to_string())
    }

    fn phantom_player(&self, handle: u32) -> Option<u32> {
        bots::phantom::player(bots::phantom::PhantomHandle(handle))
    }

    fn brain_view(&self, bot: Bot) -> Option<BotBrain> {
        bots::phantom::global()
            .brain_view(bots::phantom::PhantomHandle(bot.handle))
            .map(brain_view)
    }

    fn clankers(&self, guest: Option<&dyn engine::GuestMemory>) -> Vec<ClankerView> {
        bots::phantom::list()
            .into_iter()
            .filter_map(|bot| {
                let brain = brain_view(bots::phantom::global().brain_view(bot.handle)?);
                Some(ClankerView {
                    name: bot.identity.gamertag,
                    kind: PlayerKind::Phantom,
                    player: bot.player.map(|id| format!("player.{id}")),
                    aim: guest
                        .and_then(|m| bots::phantom::global().aim_view(m, bot.handle))
                        .map(|a| agent_proto::sim::PhantomAim {
                            tick: a.tick,
                            stick: a.stick,
                            arrived_stick: a.arrived_stick,
                            trigger: a.trigger,
                            arrived_trigger: a.arrived_trigger,
                            action_deg: a.action_deg,
                            fire_primary: a.fire_primary,
                            assist_strength: a.assist_strength,
                            unit_forward: a.unit_forward,
                            assist_player: a.assist_player.map(|i| format!("player.{i}")),
                        }),
                    brain,
                })
            })
            .collect()
    }

    fn remove(&mut self, bot: Bot) -> Result<(), String> {
        bots::phantom::remove(bots::phantom::PhantomHandle(bot.handle)).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phantom_goes_on_the_process_wide_books() {
        // Pending until a hosted game runs (docs/game/bots.md §5.1).
        let mut b = SimBots::new(false);
        assert!(
            b.add_phantom(
                &Identity {
                    xuid: "000e0000c0ffee01".into(),
                    gamertag: "Clanker".into(),
                },
                Some(1),
                PadSource::Brain,
            )
            .unwrap_err()
            .contains("--host-bots")
        );
        let mut b = SimBots::new(true);
        let h = b
            .add_phantom(
                &Identity {
                    xuid: "000e0000c0ffee01".into(),
                    gamertag: "Clanker".into(),
                },
                Some(1),
                PadSource::Brain,
            )
            .unwrap();
        assert_eq!(b.phantom_player(h), None, "no player outside a game");
        b.remove(Bot { handle: h }).unwrap();
        assert!(
            b.add_phantom(
                &Identity {
                    xuid: "zz".into(),
                    gamertag: "x".into()
                },
                None,
                PadSource::Brain,
            )
            .unwrap_err()
            .contains("hex")
        );
    }

    #[test]
    fn a_script_supplies_its_pad_to_the_phantom() {
        use pad_source::PadSource as _;

        let guest = engine::testing::FakeGuest::new();
        let remote = Remote::new();
        let mut pad = Gamepad::default();
        pad.buttons = input::gamepad::Buttons::START;
        remote.replace([input::backend::remote::Step {
            pad,
            ..Default::default()
        }]);
        let mut source = ScriptPad(remote);
        assert!(
            source
                .pad(&Frame {
                    frame: 0,
                    tick: 100,
                    machine: [7; 6],
                    guest: &guest,
                })
                .buttons
                .contains(input::host::HostButton::Start)
        );
    }
}
