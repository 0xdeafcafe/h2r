use super::*;
use h1objects::{h1::Trigger, parity::Gun};

impl Case {
    fn firing(&mut self, object: &Value, ticks: u64, steps: Value) -> Result<(Value, Vec<Value>)> {
        let v = self.call(
            agent_proto::probe::method::WEAPON,
            json!({"player":0,"object":object,"ticks":ticks,"steps":steps}),
        )?;
        let rows = v["ticks"].as_array().context("missing captured ticks")?;
        ensure!(rows.len() == ticks as usize, "incomplete capture");
        Ok((v["initial"].clone(), rows.clone()))
    }

    pub(super) fn gun(&mut self, tag: &Tag) -> Result<()> {
        let reference = h1objects::parity::gun(&self.h1, tag)?;
        let w = &reference.weapon;
        if w.triggers.is_empty() {
            let name = self.weapon_name(&tag.name)?;
            let made = self.call(agent_proto::sim::method::GIVE, json!({"player":"player.0","weapon":name}))?;
            self.wait((w.ready_time * self.rules.hz).ceil() as u64 + 8)?;
            let (_, rows) = self.firing(
                &made["object"],
                45,
                json!([{"ticks":20,"rt":1.},{"ticks":20,"lt":1.}]),
            )?;
            let count = rows
                .iter()
                .map(|r| r["shots"].as_array().context("no shot list").map(Vec::len))
                .sum::<Result<usize>>()?;
            self.table.row(
                &format!("{} triggerless weapon", tag.name),
                0,
                count,
                count == 0,
            );
            return Ok(());
        }
        let name = self.weapon_name(&tag.name)?;
        for (index, t) in w.triggers.iter().enumerate() {
            let check = format!("{} barrel {index}", tag.name);
            if let Err(e) = self.barrel(&tag.name, &name, index, t, &reference) {
                self.table.error(&check, e);
            }
        }
        Ok(())
    }

    fn barrel(
        &mut self,
        source: &str,
        name: &str,
        index: usize,
        t: &Trigger,
        r: &Gun,
    ) -> Result<()> {
        let id = format!("{source} barrel {index}");
        let hz = f64::from(self.rules.hz);
        let made = self.call(agent_proto::sim::method::GIVE, json!({"player":"player.0","weapon":name}))?;
        let object = made["object"].as_str().context("give returned no weapon")?;
        self.wait((f64::from(r.weapon.ready_time) * hz).ceil() as u64 + 8)?;
        let player = self.player()?;
        let actual = player["weapon"].as_str().context("no held weapon")?;
        let expected = made["weapon"]["tag"]
            .as_str()
            .context("given tag")?
            .trim_start_matches("0x");
        ensure!(
            actual == expected,
            "given {name} ({expected}), player holds {actual}"
        );
        let obj = json!(object);
        let mag = usize::try_from(t.magazine)
            .ok()
            .and_then(|n| r.weapon.magazines.get(n));
        let charge = r.charge_seconds[0];
        let charged = charge > 0. && index > 0;
        let semiauto = t.flags & u32::from(self.rules.press_mask) != 0;
        let trigger = if index == 0 || charged { "rt" } else { "lt" };
        let step = |n| {
            let mut s = json!({"ticks":n});
            s[trigger] = json!(1.);
            s
        };
        let sample_ticks = 140;
        let pressed = if charge > 0. {
            if charged {
                (f64::from(charge) * hz).ceil() as u64 + 2
            } else {
                1
            }
        } else {
            sample_ticks - 2
        };
        ensure!(
            pressed + 2 <= sample_ticks,
            "charge requires a longer capture"
        );
        let (initial, tap) = self.firing(&obj, sample_ticks, json!([step(pressed),{"ticks":2}]))?;
        let shots = shots_for_barrel(&tap, index)?;
        let shot_ticks: Vec<_> = shots.iter().map(|(tick, _)| *tick).collect();
        let mode_ok = if semiauto || charge > 0. {
            shots.len() == 1
        } else {
            shots.len() > 1
        };
        self.table.row(
            &format!("{id} hold"),
            if charged {
                "charged release"
            } else if charge > 0. {
                "tap/release"
            } else if semiauto {
                "one per press"
            } else {
                "repeat"
            },
            format!("{shot_ticks:?}"),
            mode_ok,
        );
        ensure!(!shots.is_empty(), "no firing events for this barrel");
        let counts: Vec<_> = shots
            .iter()
            .map(|(_, s)| s["projectiles"].as_array().map(Vec::len))
            .collect();
        self.table.row(
            &format!("{id} projectiles per shot"),
            t.projectiles_per_shot,
            format!("{counts:?}"),
            counts
                .iter()
                .all(|n| *n == usize::try_from(t.projectiles_per_shot).ok()),
        );
        let pellets = shots
            .iter()
            .flat_map(|(_, s)| s["projectiles"].as_array().into_iter().flatten())
            .collect::<Vec<_>>();
        let mut cones = Vec::new();
        let mut matched = !pellets.is_empty();
        if source == RIFLE && index == 0 {
            let first = pellets
                .first()
                .and_then(|p| p["definition"].as_u64())
                .context("rifle projectile definition missing")?;
            self.rifle_projectile = Some(u32::try_from(first)?);
        }
        let mut perturbed = false;
        for p in pellets {
            let cone = p["cone"].as_f64().context("missing applied cone")? as f32;
            let bloom = p["bloom"]
                .as_f64()
                .context("missing barrel error fraction")? as f32;
            let [lo, hi] = t.error_angle;
            let expected = (hi - lo) * bloom + lo;
            matched &= cone == expected && (0.0..=1.0).contains(&bloom);
            let angle = p["angle"]
                .as_f64()
                .context("missing launched direction angle")?;
            let limit = t.distribution_angle.abs() + cone.abs().max(t.minimum_error);
            matched &= angle.cos() as f32 >= limit.cos();
            perturbed |= (angle.cos() as f32) < 1.;
            cones.push(cone);
        }
        if counts.iter().flatten().sum::<usize>() > 1 && cones.iter().any(|&c| c > 0.) {
            matched &= perturbed;
        }
        self.table.row(
            &format!("{id} applied spread"),
            format!("{:?} rad, by live barrel fraction", t.error_angle),
            format!(
                "{:?}..{:?}",
                cones.iter().copied().reduce(f32::min),
                cones.iter().copied().reduce(f32::max)
            ),
            matched,
        );
        if !semiauto
            && charge == 0.
            && t.error_angle[1] > t.error_angle[0]
            && t.error_acceleration_time > 0.
        {
            let end = (f64::from(t.error_acceleration_time) * hz).ceil() as usize + shot_ticks[0];
            if let Some((_, s)) = shots.iter().find(|(tick, _)| *tick >= end) {
                self.table.row(
                    &format!("{id} sustained spread"),
                    t.error_angle[1],
                    &s["projectiles"][0]["cone"],
                    s["projectiles"][0]["cone"].as_f64().map(|f| f as f32)
                        == Some(t.error_angle[1]),
                );
            } else {
                self.table.row(
                    &format!("{id} sustained spread"),
                    end,
                    "insufficient sustained fire",
                    false,
                );
            }
        }
        if !semiauto && charge == 0. {
            let spin_ticks = f64::from(t.firing_acceleration_time) * hz;
            let mut intervals = Vec::new();
            let mut ok = true;
            for pair in shot_ticks.windows(2) {
                let elapsed = pair[0] - shot_ticks[0];
                let last = &tap[pair[0]];
                if last["loaded"].as_i64() == Some(0) && mag.is_some() {
                    continue;
                }
                if last["heat"]
                    .as_f64()
                    .is_some_and(|h| h >= f64::from(r.weapon.overheated_threshold) && h > 0.)
                {
                    continue;
                }
                let progress = if spin_ticks > 0. {
                    (elapsed as f64 / spin_ticks).min(1.)
                } else {
                    1.
                };
                let [lo, hi] = t.rounds_per_second.map(f64::from);
                let rate = lo + (hi - lo) * progress;
                if rate <= 0. {
                    continue;
                }
                let expected = hz / rate;
                let interval = pair[1] - pair[0];
                intervals.push(interval);
                ok &= (interval as f64 - expected).abs() <= 1.;
            }
            self.table.row(
                &format!("{id} cadence"),
                format!(
                    "{:?} r/s over {} s",
                    t.rounds_per_second, t.firing_acceleration_time
                ),
                format!("{intervals:?} ticks"),
                ok && !intervals.is_empty(),
            );
        }
        let first = shot_ticks[0];
        let previous = if first == 0 {
            &initial
        } else {
            &tap[first - 1]
        };
        let delta = tap[first]["heat"].as_f64().context("no heat")?
            - previous["heat"].as_f64().context("no initial heat")?;
        let generated = f64::from(t.heat_per_round);
        let loss = f64::from(r.weapon.heat_loss_per_second) / hz;
        self.table.row(
            &format!("{id} heat"),
            t.heat_per_round,
            format!("{delta} on first shot"),
            (generated - loss).max(0.) <= delta && delta <= generated,
        );
        if semiauto && charge == 0. && t.rounds_per_second[1] > 0. {
            let fresh = self.call(agent_proto::sim::method::GIVE, json!({"player":"player.0","weapon":name}))?;
            self.wait((f64::from(r.weapon.ready_time) * hz).ceil() as u64 + 8)?;
            let interval = hz / f64::from(t.rounds_per_second[1]);
            let ticks = ((interval.ceil() as u64 + 2) * 3).clamp(12, 288);
            let steps: Vec<_> = (0..ticks)
                .map(|n| {
                    if n % 2 == 0 {
                        step(1)
                    } else {
                        json!({"ticks":1})
                    }
                })
                .collect();
            let (_, rapid) = self.firing(&fresh["object"], ticks, json!(steps))?;
            let fired = shots_for_barrel(&rapid, index)?;
            let gaps: Vec<_> = fired
                .windows(2)
                .filter(|w| mag.is_none() || rapid[w[0].0]["loaded"].as_i64() != Some(0))
                .map(|w| w[1].0 - w[0].0)
                .collect();
            self.table.row(
                &format!("{id} repeated-press cadence"),
                format!("{interval} ticks"),
                format!("{gaps:?}"),
                !gaps.is_empty()
                    && gaps
                        .iter()
                        .all(|&n| n as f64 >= interval.floor() && n as f64 <= interval.ceil() + 1.),
            );
        }
        // A separate release-and-repress catches a gun that fired only
        // once because it jammed. Give a fresh weapon to reset heat and ammo.
        let fresh = self.call(agent_proto::sim::method::GIVE, json!({"player":"player.0","weapon":name}))?;
        self.wait((f64::from(r.weapon.ready_time) * hz).ceil() as u64 + 8)?;
        let pulse = if charged { pressed } else { 1 };
        let gap = t.rounds_per_second[1];
        let gap = if gap > 0. {
            (hz / f64::from(gap)).ceil() as u64 + 3
        } else {
            30
        };
        ensure!(2 * (pulse + gap) <= 290, "tap cycle exceeds probe window");
        let (tap_initial, taps) = self.firing(
            &fresh["object"],
            2 * (pulse + gap),
            json!([step(pulse),{"ticks":gap},step(pulse),{"ticks":gap}]),
        )?;
        let repeated = shots_for(&taps, index)?;
        self.table
            .row(&format!("{id} two taps"), 2, repeated, repeated == 2);
        if let Some(m) = mag {
            let before = tap_initial["loaded"]
                .as_i64()
                .context("no initial magazine")?;
            let after = taps.last().context("empty trace")?["loaded"]
                .as_i64()
                .context("no magazine")?;
            let expected = before - repeated as i64 * i64::from(t.rounds_per_shot);
            self.table.row(
                &format!("{id} rounds after taps"),
                expected,
                after,
                after == expected,
            );
            self.table.row(
                &format!("{id} fresh magazine"),
                m.rounds_loaded_maximum,
                before,
                before == i64::from(m.rounds_loaded_maximum),
            );
            let reserve = tap_initial["reserve"]
                .as_i64()
                .context("no reserve count")?;
            let expected_reserve = i64::from(
                m.rounds_total_initial
                    .saturating_sub(m.rounds_loaded_maximum)
                    .max(0),
            );
            self.table.row(
                &format!("{id} initial reserve"),
                expected_reserve,
                reserve,
                reserve == expected_reserve,
            );
            self.reload(&id, &fresh["object"], m, hz)?;
        }
        Ok(())
    }

    fn reload(
        &mut self,
        id: &str,
        obj: &Value,
        m: &h1objects::h1::Magazine,
        hz: f64,
    ) -> Result<()> {
        let loaded = (m.rounds_loaded_maximum - m.rounds_reloaded.max(1)).max(0);
        self.call(
            agent_proto::sim::method::STATE_SET,
            json!({"player":"player.0","ammo":{"loaded":loaded,"reserve":m.rounds_loaded_maximum}}),
        )?;
        let expected = f64::from(m.reload_time) * hz;
        let (initial, samples) = self.firing(
            obj,
            (expected.ceil() as u64 + 40).min(290),
            json!([{"buttons":["x"],"ticks":2}]),
        )?;
        let start = samples
            .iter()
            .position(|r| r["magazine_state"].as_u64().is_some_and(|s| s != 0));
        let end = samples
            .iter()
            .position(|r| r["loaded"].as_i64().is_some_and(|n| n > i64::from(loaded)));
        let elapsed = start.zip(end).and_then(|(a, b)| b.checked_sub(a));
        self.table.row(
            &format!("{id} reload"),
            format!("{expected} ticks"),
            json!(elapsed),
            elapsed.is_some_and(|t| (t as f64 - expected).abs() <= 1.),
        );
        self.table.row(
            &format!("{id} reloaded magazine"),
            m.rounds_loaded_maximum,
            samples.last().context("empty reload")?["loaded"].clone(),
            samples.last().unwrap()["loaded"] == m.rounds_loaded_maximum,
        );
        let final_state = samples.last().context("empty reload trace")?;
        let total = |row: &Value| -> Result<i64> {
            Ok(row["loaded"].as_i64().context("loaded rounds")?
                + row["reserve"].as_i64().context("reserve rounds")?)
        };
        let initial_total = total(&initial)?;
        let final_total = total(final_state)?;
        self.table.row(
            &format!("{id} reload conserves ammunition"),
            initial_total,
            final_total,
            initial_total == final_total,
        );
        Ok(())
    }
}

fn shots_for_barrel(rows: &[Value], index: usize) -> Result<Vec<(usize, &Value)>> {
    let mut out = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        for s in row["shots"].as_array().context("missing barrel events")? {
            if s["barrel"].as_u64() == Some(index as u64) {
                out.push((n, s));
            }
        }
    }
    Ok(out)
}
fn shots_for(rows: &[Value], index: usize) -> Result<usize> {
    Ok(shots_for_barrel(rows, index)?.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trigger_counts_follow_engine_events_including_multiple_shots_on_one_tick() {
        let rows = [
            json!({"shots":[{"barrel":1}]}),
            json!({"shots":[]}),
            json!({"shots":[{"barrel":0},{"barrel":0}]}),
        ];
        let events = shots_for_barrel(&rows, 0).unwrap();
        assert_eq!(
            events.iter().map(|(tick, _)| *tick).collect::<Vec<_>>(),
            [2, 2]
        );
        assert!(shots_for_barrel(&[json!({})], 0).is_err());
    }
}
