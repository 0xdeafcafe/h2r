//! Script-visible state and the worker that plays the cast's lines.

use h1ai::conversations::Line;

use super::{staging::Staging, *};

impl Scene<'_, '_, '_, '_> {
    pub(super) fn in_status(&self, status: i32) -> Expr {
        eq(self.status.get(), short(status))
    }

    fn new_request(&self) -> Expr {
        not(eq(self.working.get(), self.request.get()))
    }

    pub(super) fn active(&self) -> Expr {
        and([
            pred(">", [self.status.get(), short(0)]),
            pred("<", [self.status.get(), short(FAILED)]),
        ])
    }

    fn stop_sound(&self) -> Expr {
        bridge(Op::StopSound, [self.instance.operand(), 0, 0])
    }

    fn finish(&self, status: i32, staging: &Staging) -> Expr {
        begin(
            h2t::VOID,
            [
                self.stop_sound(),
                self.sound.set(Expr::None(SOUND)),
                self.line.set(short(NO_LINE)),
                self.status.set(short(status)),
            ]
            .into_iter()
            .chain(staging.stop()),
        )
    }

    pub(super) fn build(mut self) -> Result<Entry> {
        let cast = self.cast()?;
        let staging = Staging::build(&mut self)?;
        let required_body = self.required_alive();
        let near_body = staging.ready(&self);
        let advance_body = when(self.in_status(HOLD), self.advanced.set(boolean(true)));
        let stop_body = when(self.active(), self.finish(ABORTED, &staging));
        let running_body = and([
            not(self.new_request()),
            or([self.in_status(READY), self.in_status(PLAYING)]),
        ]);
        let required = self.program.script("required", 3, required_body)?;
        let near = self.program.script("near", 3, near_body)?;
        let advance = self.program.script("advance", 3, advance_body)?;
        let stop = self.program.script("stop", 3, stop_body)?;
        let running = self.program.script("running", 3, running_body)?;
        let mut names = Vec::new();
        for (participant, role) in self.source.participants.iter().zip(&self.roles) {
            if let Some(role) = role {
                let mut publish = vec![bridge(
                    Op::RememberVoice,
                    [
                        role.unit.operand(),
                        role.voice.operand(),
                        self.scratch.profile_base,
                    ],
                )];
                if let Some(alias) = self.program.alias(participant.assigned_name.as_deref()) {
                    publish.push(alias.unit.set(role.unit.get()));
                }
                names.push(when(alive(role.unit.get()), begin(h2t::VOID, publish)));
            }
        }
        let publish = self.program.script("publish", 3, begin(h2t::VOID, names))?;
        let enqueue = |status| {
            let mut body = vec![self.status.set(short(status))];
            if status == READY {
                body.push(publish.call());
            }
            body.push(boolean(true));
            begin(h2t::BOOLEAN, body)
        };
        let failed = begin(
            h2t::BOOLEAN,
            [self.finish(FAILED, &staging), boolean(false)],
        );
        let wait_for_player = if self.source.delays_start() {
            enqueue(TRYING)
        } else {
            failed.clone()
        };
        let reset = self.request.set(add_one(h2t::LONG, self.request.get()));
        let start_body = either(
            self.active(),
            boolean(true),
            begin(
                h2t::BOOLEAN,
                [
                    reset,
                    self.line.set(short(-1)),
                    self.advanced.set(boolean(false)),
                    cast.call(),
                    either(
                        required.call(),
                        either(near.call(), enqueue(READY), wait_for_player),
                        failed,
                    ),
                ],
            ),
        );
        let start = self.program.script("start", 3, start_body)?;

        let mut play = vec![
            when(self.in_status(TRYING), publish.call()),
            self.status.set(short(READY)),
        ];
        play.extend(staging.start());
        for (i, line) in self.source.lines.iter().enumerate() {
            if line.speaker == -1 {
                continue;
            }
            let step = self.play_line(i, line, &running, &staging)?;
            let ready = when(
                running.call(),
                begin(h2t::VOID, [self.status.set(short(PLAYING)), step]),
            );
            let wait = staging.wait(line, &running);
            play.push(when(
                running.call(),
                begin(h2t::VOID, wait.into_iter().chain([ready])),
            ));
        }
        play.push(when(running.call(), self.finish(DONE, &staging)));
        let mut worker = vec![
            wait_until(or([self.in_status(TRYING), self.in_status(READY)]), 1),
            self.working.set(self.request.get()),
        ];
        if self.source.delays_start() {
            worker.push(when(
                self.in_status(TRYING),
                wait_until(
                    or([
                        self.new_request(),
                        not(self.in_status(TRYING)),
                        not(required.call()),
                        near.call(),
                    ]),
                    30,
                ),
            ));
        }
        worker.push(when(
            and([
                not(self.new_request()),
                or([self.in_status(TRYING), self.in_status(READY)]),
            ]),
            either(
                and([required.call(), or([self.in_status(READY), near.call()])]),
                begin(h2t::VOID, play),
                self.finish(FAILED, &staging),
            ),
        ));
        self.program.script("worker", 2, begin(h2t::VOID, worker))?;

        Ok(Entry {
            start,
            stop,
            status: self.status,
            line: self.line,
            advance,
        })
    }

    fn play_line(
        &self,
        index: usize,
        line: &Line,
        running: &Script,
        staging: &Staging,
    ) -> Result<Expr> {
        let role_index = line.speaker as usize;
        let participant = self.source.participants.get(role_index).ok_or_else(|| {
            invalid(format!(
                "{}: line {index} has invalid speaker {}",
                self.source.name, line.speaker
            ))
        })?;
        let role = self.roles[role_index].as_ref();
        let unit = || role.map_or(Expr::None(OBJECT), |role| role.unit.get());
        let emitter = role
            .filter(|_| !participant.nonspatial())
            .map_or(u32::MAX, |role| role.unit.operand());
        let mut sound = line
            .sounds
            .iter()
            .flatten()
            .next()
            .map_or(Expr::None(SOUND), |name| Expr::Hook(SOUND, name.clone()));
        if let Some(role) = role {
            for (voice, alternative) in participant.voices.iter().zip(&line.sounds).rev() {
                if *voice >= 0 {
                    let take = alternative
                        .as_ref()
                        .map_or(Expr::None(SOUND), |name| Expr::Hook(SOUND, name.clone()));
                    sound = either(eq(role.voice.get(), short(i32::from(*voice))), take, sound);
                }
            }
        }
        let mut play = vec![self.line.set(short(index as i32)), self.sound.set(sound)];
        let (face, release): (Vec<_>, Vec<_>) = staging.face(self, line).into_iter().unzip();
        play.extend(face);
        let lost = || role.map_or(boolean(false), |_| not(alive(unit())));
        let ended = begin(
            h2t::BOOLEAN,
            [
                bridge(Op::PollSound, [self.instance.operand(), 0, 0]),
                eq(self.instance.get(), long(u32::MAX)),
            ],
        );
        play.push(when(
            not(eq(self.sound.get(), Expr::None(SOUND))),
            begin(
                h2t::VOID,
                [
                    bridge(
                        Op::StartSound,
                        [self.sound.operand(), emitter, self.instance.operand()],
                    ),
                    wait_until(or([not(running.call()), lost(), ended]), 1),
                ],
            ),
        ));
        let mut after = release.clone();
        after.push(self.sound.set(Expr::None(SOUND)));
        let pause = (line.pause_after * 30.0)
            .round()
            .clamp(0.0, i16::MAX as f32) as i32;
        if pause > 0 {
            after.push(call(
                "sleep_until",
                h2t::VOID,
                [
                    not(running.call()),
                    short(1),
                    Expr::Number(h2t::LONG, pause),
                ],
            ));
        }
        if line.waits_for_advance() {
            after.push(when(
                running.call(),
                begin(
                    h2t::VOID,
                    [
                        self.status.set(short(HOLD)),
                        wait_until(
                            or([
                                self.new_request(),
                                not(self.in_status(HOLD)),
                                self.advanced.get(),
                            ]),
                            1,
                        ),
                        when(
                            and([not(self.new_request()), self.in_status(HOLD)]),
                            begin(
                                h2t::VOID,
                                [
                                    self.advanced.set(boolean(false)),
                                    self.status.set(short(PLAYING)),
                                ],
                            ),
                        ),
                    ],
                ),
            ));
        }
        let after = begin(h2t::VOID, after);
        if role.is_none() {
            play.push(when(running.call(), after));
            return Ok(begin(h2t::VOID, play));
        }
        let absent = if participant.flags & 7 != 0 {
            begin(
                h2t::VOID,
                release
                    .into_iter()
                    .chain([self.stop_sound(), self.sound.set(Expr::None(SOUND))]),
            )
        } else {
            self.finish(ABORTED, staging)
        };
        play.push(when(running.call(), either(lost(), absent.clone(), after)));
        Ok(either(lost(), absent, begin(h2t::VOID, play)))
    }
}
