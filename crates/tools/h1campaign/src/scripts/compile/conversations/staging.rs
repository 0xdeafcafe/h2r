//! The conversation's temporary command scripts run on the cast's native AI.

use crate::ai::conversations::{Addressee, Line};

use super::*;

#[derive(Clone)]
struct Facing {
    target: Global,
    command: Script,
}

#[derive(Clone)]
struct Actor {
    unit: Global,
    ai: Global,
    facing: Option<Facing>,
}

pub(super) struct Staging {
    player: Option<Global>,
    approach: Option<Script>,
    cast: Vec<Option<Actor>>,
    focus: Option<Script>,
}

fn exists(ty: u16, value: Expr) -> Expr {
    not(eq(value, Expr::None(ty)))
}

impl Actor {
    fn run(&self, command: &Script) -> Expr {
        begin(
            h2t::VOID,
            [
                bridge(Op::Actor, [self.unit.operand(), self.ai.operand(), 0]),
                when(
                    exists(AI, self.ai.get()),
                    call(
                        "cs_stack_command_script",
                        h2t::VOID,
                        [self.ai.get(), command.command()],
                    ),
                ),
            ],
        )
    }

    fn approaching(&self, command: &Script) -> Expr {
        and([
            alive(self.unit.get()),
            exists(AI, self.ai.get()),
            or(["cs_command_script_running", "cs_command_script_queued"]
                .map(|name| pred(name, [self.ai.get(), command.command()]))),
        ])
    }

    fn cancel(&self, command: &Script) -> Expr {
        bridge(
            Op::CancelCommand,
            [self.unit.operand(), u32::from(command.index), 0],
        )
    }

    fn stop_facing(&self) -> Option<Expr> {
        self.facing.as_ref().map(|f| self.cancel(&f.command))
    }
}

impl Staging {
    pub(super) fn build(scene: &mut Scene<'_, '_, '_, '_>) -> Result<Self> {
        let source = scene.source;
        let physical = scene.roles.iter().flatten().count();
        let needs_player = source.requires_player_attention()
            || source.runs_to_player()
            || source.lines.iter().any(|line| {
                scene
                    .roles
                    .iter()
                    .enumerate()
                    .any(|(i, role)| role.is_some() && line.faces(i) == Some(Addressee::Player))
            });
        let player = if needs_player {
            Some(scene.program.global("focus_player", Expr::None(OBJECT))?)
        } else {
            None
        };
        let focus = if let Some(player) = &player {
            let first = scene
                .roles
                .iter()
                .flatten()
                .next()
                .map_or(0, |r| r.unit.operand());
            let flags = physical as u32
                | if source.requires_player_attention() {
                    0x100
                } else {
                    0
                };
            Some(scene.program.script(
                "focus",
                3,
                bridge(Op::Focus, [first, player.operand(), flags]),
            )?)
        } else {
            None
        };
        let approach = if let Some(player) = &player
            && source.runs_to_player()
            && physical != 0
        {
            let distance = source.run_to_player_distance;
            let body = when(
                and([scene.active(), alive(player.get())]),
                call(
                    "cs_approach",
                    h2t::VOID,
                    [
                        player.get(),
                        Expr::Real(distance),
                        Expr::Real(distance),
                        Expr::Real(distance),
                    ],
                ),
            );
            Some(scene.program.script("approach", 5, body)?)
        } else {
            None
        };
        let mut cast = Vec::with_capacity(scene.roles.len());
        for (index, role) in scene.roles.iter().enumerate() {
            let has_target = role.is_some()
                && source.lines.iter().any(|line| match line.faces(index) {
                    Some(Addressee::Player) => true,
                    Some(Addressee::Role(other)) => {
                        scene.roles.get(other).is_some_and(Option::is_some)
                    }
                    None => false,
                });
            let actor = if let Some(role) = role
                && (approach.is_some() || has_target)
            {
                let ai = scene.program.global("role_ai", Expr::None(AI))?;
                let facing = if has_target {
                    let target = scene.program.global("face_target", Expr::None(OBJECT))?;
                    let valid = and([scene.in_status(PLAYING), alive(target.get())]);
                    let body = when(
                        valid.clone(),
                        begin(
                            h2t::VOID,
                            [
                                call("cs_face_object", h2t::VOID, [boolean(true), target.get()]),
                                call("cs_look_object", h2t::VOID, [boolean(true), target.get()]),
                                wait_until(not(valid), 1),
                            ],
                        ),
                    );
                    let command = scene.program.script("face", 5, body)?;
                    Some(Facing { target, command })
                } else {
                    None
                };
                Some(Actor {
                    unit: role.unit.clone(),
                    ai,
                    facing,
                })
            } else {
                None
            };
            cast.push(actor);
        }
        Ok(Self {
            player,
            approach,
            cast,
            focus,
        })
    }

    pub(super) fn ready(&self, scene: &Scene<'_, '_, '_, '_>) -> Expr {
        let attention = scene
            .source
            .requires_player_attention()
            .then(|| exists(OBJECT, self.player.as_ref().unwrap().get()));
        let ready = and(attention.into_iter().chain([scene.near_player()]));
        self.focus.as_ref().map_or(ready.clone(), |focus| {
            begin(h2t::BOOLEAN, [focus.call(), ready])
        })
    }

    pub(super) fn start(&self) -> impl Iterator<Item = Expr> + '_ {
        self.approach.iter().flat_map(move |script| {
            self.cast
                .iter()
                .flatten()
                .map(move |actor| actor.run(script))
        })
    }

    pub(super) fn stop(&self) -> impl Iterator<Item = Expr> + '_ {
        self.cast.iter().flatten().flat_map(move |actor| {
            actor
                .stop_facing()
                .into_iter()
                .chain(self.approach.iter().map(move |script| actor.cancel(script)))
        })
    }

    pub(super) fn wait(&self, line: &Line, running: &Script) -> Option<Expr> {
        let command = self.approach.as_ref()?;
        let waiting = if line.waits_for_cast() {
            or(self
                .cast
                .iter()
                .flatten()
                .map(|actor| actor.approaching(command)))
        } else if line.waits_for_speaker() {
            self.cast
                .get(line.speaker as usize)?
                .as_ref()?
                .approaching(command)
        } else {
            return None;
        };
        Some(wait_until(or([not(running.call()), not(waiting)]), 1))
    }

    pub(super) fn face(&self, scene: &Scene<'_, '_, '_, '_>, line: &Line) -> Vec<(Expr, Expr)> {
        self.cast
            .iter()
            .enumerate()
            .filter_map(|(index, actor)| {
                let actor = actor.as_ref()?;
                let face = actor.facing.as_ref()?;
                let target = match line.faces(index)? {
                    Addressee::Player => self.player.as_ref()?,
                    Addressee::Role(index) => &scene.roles.get(index)?.as_ref()?.unit,
                };
                let start = when(
                    and([alive(actor.unit.get()), alive(target.get())]),
                    begin(
                        h2t::VOID,
                        [face.target.set(target.get()), actor.run(&face.command)],
                    ),
                );
                Some((start, actor.cancel(&face.command)))
            })
            .collect()
    }
}
