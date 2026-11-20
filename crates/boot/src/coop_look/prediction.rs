//! A 0x28 action already carries absolute facing from retail player control.
//! Replaying absolute assignments is equivalent to taking the last pending
//! one. Keep earlier sends to identify which look the host actually consumed.
use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};

use serde::{Deserialize, Serialize};

pub(super) const MAX_PENDING: usize = 64;
const MAX_UPDATE_AGE: u32 = 30;
const SNAP: f32 = PI / 6.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct Aim {
    pub yaw: f32,
    pub pitch: f32,
}

impl Aim {
    pub fn difference(self, other: Self) -> Self {
        let yaw = self.yaw - other.yaw;
        Self {
            yaw: if yaw.abs() > PI {
                (yaw + PI).rem_euclid(TAU) - PI
            } else {
                yaw
            },
            pitch: self.pitch - other.pitch,
        }
    }

    fn size(self) -> f32 {
        self.yaw.hypot(self.pitch)
    }

    pub fn plus(self, d: Self) -> Self {
        Self {
            yaw: (self.yaw + d.yaw).rem_euclid(TAU),
            pitch: self.pitch + d.pitch,
        }
    }

    // 0x86f90 writes unsigned 13-bit yaw and 12-bit pitch. Compare
    // within a wire cell on each axis, including yaw's 0/2pi seam.
    fn same_on_wire(self, other: Self) -> bool {
        let d = self.difference(other);
        d.yaw.abs() <= TAU / 8191.0 && d.pitch.abs() <= TAU / 4095.0
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Sent {
    seq: u32,
    after: u32,
    aim: Aim,
}

fn newer(n: u32, old: u32) -> bool {
    (n.wrapping_sub(old) as i32) > 0
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Prediction {
    queue: VecDeque<Sent>,
    pub update: Option<u32>,
    pub confirmed: Option<Aim>,
    pub ack_seq: Option<u32>,
    pub ack_update: Option<u32>,
    pub dropped: u32,
    last: Option<Sent>,
    blend: Aim,
    correction: f32,
    last_adjusted: Option<Aim>,
    handoff: bool,
}

impl Prediction {
    pub fn sent(&mut self, seq: u32, after: u32, aim: Aim, enabled: bool) {
        if self.last.is_some_and(|last| !newer(seq, last.seq)) {
            return;
        }
        let sent = Sent { seq, after, aim };
        self.last = Some(sent);
        if enabled {
            if self.queue.len() == MAX_PENDING {
                self.drop_front();
            }
            self.queue.push_back(sent);
        } else {
            self.disable();
        }
    }

    pub fn last_send(&self) -> Option<(u32, Aim)> {
        self.last.map(|p| (p.seq, p.aim))
    }

    fn drop_front(&mut self) {
        self.queue.pop_front();
        self.dropped = self.dropped.saturating_add(1);
    }

    pub fn disable(&mut self) {
        self.queue.clear();
        self.blend = Aim::default();
        self.correction = 0.0;
        self.last_adjusted = None;
        self.handoff = false;
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    pub fn target(&self) -> Option<Aim> {
        self.queue.back().map(|p| p.aim).or(self.confirmed)
    }

    pub fn apply(&mut self, update: u32, actual: Option<Aim>) {
        if self.update.is_some_and(|last| !newer(update, last)) {
            return;
        }
        self.update = Some(update);
        let old = self.queue.back().map(|p| p.aim);
        if let Some(aim) = actual {
            // 0x27 echoes the action but not the 0x28 sequence. Never ack a
            // send against a host update the client had seen before sending.
            let matched = self
                .queue
                .iter()
                .position(|p| newer(update, p.after) && p.aim.same_on_wire(aim));
            if let Some(i) = matched {
                for _ in 0..i {
                    self.drop_front();
                }
                let matched = self.queue.pop_front().expect("located pending send");
                self.ack_seq = Some(matched.seq);
                self.ack_update = Some(update);
                self.correction = self.correction.max(aim.difference(matched.aim).size());
            } else if self
                .confirmed
                .is_some_and(|prev| aim.difference(prev).size() > SNAP)
            {
                while !self.queue.is_empty() {
                    self.drop_front();
                }
                self.blend = Aim::default();
            }
            self.confirmed = Some(aim);
        }
        while self.queue.front().is_some_and(|p| {
            newer(update, p.after) && update.wrapping_sub(p.after) > MAX_UPDATE_AGE
        }) {
            self.drop_front();
        }
        if old.is_some() && self.queue.is_empty() {
            self.handoff = self.last_adjusted.is_some();
        }
        if let (Some(old), Some(target)) = (old, self.target()) {
            let error = old.difference(target);
            if error.size() > 0.0 {
                self.correction = self.correction.max(error.size());
                self.blend = if error.size() < SNAP {
                    Aim {
                        yaw: error.yaw * 0.5,
                        pitch: error.pitch * 0.5,
                    }
                } else {
                    Aim::default()
                };
            }
        }
    }

    /// The first-person director may already have used some or all of the
    /// outgoing look. Add only the portion missing from the drawn camera on
    /// each turning axis. At rest its full recoil, shake and roll pass through.
    pub fn draw(&mut self, camera: Aim) -> Option<(Aim, f32)> {
        let confirmed = self.confirmed?;
        let blend = std::mem::take(&mut self.blend);
        let pending = self.target()?.plus(blend).difference(confirmed);
        let native = camera.difference(confirmed);
        // A reversal can put the native camera beyond the final angle, yet
        // still inside the turns we sent. Remove that part of the earlier
        // turn too. An excursion outside the sent arc stays camera motion.
        let mut lo = Aim::default();
        let mut hi = Aim::default();
        for movement in self
            .queue
            .iter()
            .map(|p| p.aim.difference(confirmed))
            .chain([pending])
        {
            lo.yaw = lo.yaw.min(movement.yaw);
            hi.yaw = hi.yaw.max(movement.yaw);
            lo.pitch = lo.pitch.min(movement.pitch);
            hi.pitch = hi.pitch.max(movement.pitch);
        }
        let mut offset = Aim {
            yaw: pending.yaw - native.yaw.clamp(lo.yaw, hi.yaw),
            pitch: pending.pitch - native.pitch.clamp(lo.pitch, hi.pitch),
        };
        // The applied update can precede the observer camera by a frame.
        // Acknowledge the input immediately, then ease that visual handoff
        // once, without delaying a fresh send.
        if std::mem::take(&mut self.handoff)
            && self.queue.is_empty()
            && let Some(last) = self.last_adjusted
        {
            let gap = last.difference(camera);
            self.correction = self.correction.max(gap.size());
            offset = if self.correction < SNAP {
                Aim {
                    yaw: gap.yaw * 0.5,
                    pitch: gap.pitch * 0.5,
                }
            } else {
                Aim::default()
            };
        }
        self.last_adjusted = (offset != Aim::default()).then(|| camera.plus(offset));
        Some((offset, std::mem::take(&mut self.correction)))
    }
}
