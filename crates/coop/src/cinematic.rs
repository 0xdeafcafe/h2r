//! Extra cinematic simulation ticks scheduled by the host for one frame.
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CinematicTicks(u8);
impl CinematicTicks {
    pub const MAX: u8 = 15;
    pub fn new(ticks: u8) -> Option<Self> {
        (ticks <= Self::MAX).then_some(Self(ticks))
    }
    pub fn count(self) -> u8 {
        self.0
    }
    /// World mode at simulation globals +8. Followers consume the resulting updates.
    pub fn authors_updates(world: u32) -> bool {
        matches!(world, 1 | 2)
    }
}

#[derive(Default)]
pub(crate) struct Stream {
    first: u32,
    values: VecDeque<CinematicTicks>,
}
impl Stream {
    pub fn end(&self) -> u32 {
        self.first + self.values.len() as u32
    }
    pub fn at(&self, frame: u32) -> Option<CinematicTicks> {
        self.values
            .get(frame.checked_sub(self.first)? as usize)
            .copied()
    }
    pub fn receive(&mut self, first: u32, ticks: &[CinematicTicks]) {
        if let Some(skip) = self.end().checked_sub(first) {
            self.values
                .extend(ticks.iter().skip(skip as usize).copied());
        }
    }
    pub fn discard(&mut self, before: u32) {
        let count = before
            .saturating_sub(self.first)
            .min(self.values.len() as u32);
        self.values.drain(..count as usize);
        self.first += count;
    }
    pub fn outgoing(&self, limit: usize) -> (u32, Vec<CinematicTicks>) {
        (
            self.first,
            self.values.iter().take(limit).copied().collect(),
        )
    }
}
