//! Script-visible state and the worker that plays the cast's lines.

use h1ai::conversations::Line;

use super::*;

impl Scene<'_, '_, '_, '_> {
    fn in_status(&self, status: i32) -> Expr {
        eq(self.status.get(), short(status))
    }

    fn new_request(&self) -> Expr {
        not(eq(self.working.get(), self.request.get()))
    }

    fn active(&self) -> Expr {
        and([
            pred(">", [self.status.get(), short(0)]),
            pred("<", [self.status.get(), short(FAILED)]),
        ])
    }

    fn finish(&self, status: i32) -> Expr {
        begin(
            h2t::VOID,
            [
                call("sound_impulse_stop", h2t::VOID, [self.sound.get()]),
                self.sound.set(Expr::None(SOUND)),
                self.line.set(short(NO_LINE)),
                self.status.set(short(status)),
            ],
        )
    }

    pub(super) fn build(mut self) -> Result<Entry> {
        let cast = self.cast()?;
        let required_body = self.required_alive();
        let near_body = self.near_player();
        let advance_body = when(self.in_status(HOLD), self.advanced.set(boolean(true)));
        let stop_body = when(self.active(), self.finish(ABORTED));
        let running_body = and([not(self.new_request()), self.in_status(PLAYING)]);
        let required = self.program.script("required", 3, required_body)?;
        let near = self.program.script("near", 3, near_body)?;
        let advance = self.program.script("advance", 3, advance_body)?;
        let stop = self.program.script("stop", 3, stop_body)?;
        let running = self.program.script("running", 3, running_body)?;
        let mut names = Vec::new();
        for (participant, role) in self.source.participants.iter().zip(&self.roles) {
            if let (Some(role), Some(alias)) = (
                role,
                self.program.alias(participant.assigned_name.as_deref()),
            ) {
                names.push(when(
                    alive(role.unit.get()),
                    begin(
                        h2t::VOID,
                        [
                            alias.unit.set(role.unit.get()),
                            alias.voice.set(role.voice.get()),
                        ],
                    ),
                ));
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
        let failed = begin(h2t::BOOLEAN, [self.finish(FAILED), boolean(false)]);
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
                        either(near.call(), enqueue(READY), enqueue(TRYING)),
                        failed,
                    ),
                ],
            ),
        );
        let start = self.program.script("start", 3, start_body)?;

        let mut play = vec![
            when(self.in_status(TRYING), publish.call()),
            self.status.set(short(PLAYING)),
        ];
        for (i, line) in self.source.lines.iter().enumerate() {
            if line.speaker == -1 {
                continue;
            }
            let step = self.play_line(i, line, &running)?;
            play.push(when(running.call(), step));
        }
        play.push(when(running.call(), self.finish(DONE)));
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
            either(required.call(), begin(h2t::VOID, play), self.finish(FAILED)),
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

    fn play_line(&self, index: usize, line: &Line, running: &Script) -> Result<Expr> {
        let role_index = line.speaker as usize;
        let participant = self.source.participants.get(role_index).ok_or_else(|| {
            invalid(format!(
                "{}: line {index} has invalid speaker {}",
                self.source.name, line.speaker
            ))
        })?;
        let role = self.roles[role_index].as_ref();
        let unit = || role.map_or(Expr::None(OBJECT), |role| role.unit.get());
        let emitter = if participant.nonspatial() {
            Expr::None(OBJECT)
        } else {
            unit()
        };
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
        let ended = pred(
            "<=",
            [
                call("sound_impulse_language_time", h2t::LONG, [self.sound.get()]),
                Expr::Number(h2t::LONG, 0),
            ],
        );
        let lost = || role.map_or(boolean(false), |_| not(alive(unit())));
        play.push(when(
            not(eq(self.sound.get(), Expr::None(SOUND))),
            begin(
                h2t::VOID,
                [
                    call(
                        "sound_impulse_start",
                        h2t::VOID,
                        [self.sound.get(), emitter, Expr::Real(1.0)],
                    ),
                    wait_until(or([not(running.call()), lost(), ended]), 1),
                ],
            ),
        ));

        let mut after = vec![self.sound.set(Expr::None(SOUND))];
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
                [
                    call("sound_impulse_stop", h2t::VOID, [self.sound.get()]),
                    self.sound.set(Expr::None(SOUND)),
                ],
            )
        } else {
            self.finish(ABORTED)
        };
        play.push(when(running.call(), either(lost(), absent.clone(), after)));
        Ok(either(lost(), absent, begin(h2t::VOID, play)))
    }
}
