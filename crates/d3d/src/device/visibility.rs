//! Visibility tests (occlusion queries).
//!
//! Halo's one user is the lens-flare/sun visibility pass (`0x20bb0`, and the
//! reader `0x207b0`). Per query it:
//!
//! 1. pushes `CLEAR_REPORT_VALUE` 1 and `SET_ZPASS_PIXEL_COUNT_ENABLE` 1
//!    inline (`0x20cec`, an inlined `BeginVisibilityTest`), draws through a
//!    callback whose return value is the pixel count it expects
//!    (`0x20d07`), and calls `EndVisibilityTest` (`0x3f85d0` at `0x20d0f`);
//! 2. on a later frame, reads the result. The index rotates through three
//!    sets (`frame % 3`; object `i` of set `s` is index `s + 3 * i`), and a
//!    bit per slot (`0x5233F0`, set at `0x20d51`) says a test was ended
//!    there. Two places read it:
//!    - the reader `0x207b0` (call at `0x20840`) reads set `(frame + 1) % 3`,
//!      the tests ended two frames before, divides the count by the
//!      expected one (`0x20861`) and clamps it to `[0, 1]`, then smooths
//!      the object's visibility byte towards it. On `D3DERR_TESTINCOMPLETE`
//!      (any non-zero `EAX`, `0x20847`) it skips the object: the byte keeps
//!      its last value.
//!    - the writer `0x20bb0` (call at `0x20cbb`), before it begins a new
//!      test in a slot that had one (set `frame % 3`, ended three frames
//!      before), asks for that test's result and throws the count away. On
//!      `D3DERR_TESTINCOMPLETE` (`0x20cc4`) it skips the object this frame:
//!      no new test, the old one stays in the slot.
//!
//!    Neither retries in a loop: an incomplete answer costs one frame of
//!    staleness for that object and nothing else.
//!
//! On the Xbox, `EndVisibilityTest` (`EAX` = index; `ret`) finds the
//! 16-byte report slot `index & 0xFF` of page `index >> 8` (64 pages at
//! device `+0x7D4`, allocated on first use by `0x3f8570`), stamps it with
//! the GPU time (`+0x2C`) and pushes `SET_ZPASS_PIXEL_COUNT_ENABLE` 0 and
//! `GET_REPORT` (type 1, the slot). The GPU writes `{timestamp (8 bytes),
//! count, status 0}` there when it gets that far. `GetVisibilityTestResult`
//! (`EAX` = index, `EDX` = optional `ULONGLONG*` timestamp out, `[esp+4]` =
//! `UINT*` count out; `ret 4`) returns `D3DERR_TESTINCOMPLETE` while the
//! status word is still `0xFFFFFFFF`, writing nothing, and first calls
//! `D3D_SetFence(0)` (`0x3f8660`) when the GPU time has not moved since
//! the end (`0x3f8659`), so the work reaches the GPU. Otherwise it copies
//! the count and timestamp out and returns 0.
//!
//! The pages are D3D-private: only `0x3f8570`, `0x3f8630` and the teardown
//! (`0x3fc69d`) touch `+0x7D4` (checked by the ignored image test in
//! `tests/round7.rs`). So the device keeps the slots itself, by index,
//! and never allocates guest pages.
//!
//! **Results.** The device runs the push buffer synchronously (see "GPU
//! model" in `docs/graphics/d3d.md`), so a test that has been ended is in the
//! backend's queue. Whether its count is back depends on the backend: a
//! render thread ([`crate::ThreadedBackend`]) may not have reached it yet,
//! and a GPU may not have finished it.
//!
//! - **Polling** (the default). The device asks the backend without waiting
//!   ([`Backend::try_pixel_count`]). A count that is back is answered. One
//!   that is not, for a test ended at most [`VISIBILITY_PENDING_SWAPS`]
//!   `Swap`s ago, is answered `D3DERR_TESTINCOMPLETE`, with the original's
//!   fence kick. Older tests wait ([`Backend::read_pixel_count`]): the
//!   hardware's GPU is never that far behind, so the game never sees an
//!   older test incomplete. Halo reads at two and three frames, so in
//!   steady state the count is back and nothing waits; when the render
//!   thread lags, the reader keeps last frame's value as on the hardware.
//!   Which calls answer incomplete depends on host timing.
//! - **Waiting** (`H2_D3D_VISIBILITY_POLL=0`, [`Device::set_visibility_poll`]).
//!   Every call waits for the count, so `D3DERR_TESTINCOMPLETE` is never
//!   returned and the answers do not depend on host timing. With a render
//!   thread the wait is for everything queued before it.
//!
//! Either way, a count once answered is kept, so asking again answers the
//! same, as the slot would.

use super::*;

/// `D3DERR_TESTINCOMPLETE`: what the original returns while the report is
/// not written (see the module docs).
pub const D3DERR_TESTINCOMPLETE: u32 = 0x8876_0828;

/// Polling answers `D3DERR_TESTINCOMPLETE` only for a test ended at most
/// this many `Swap`s ago; an older one waits for its count. Halo's reader
/// asks after two (so it may see incomplete, and keeps last frame's value),
/// its writer after three (so it never does).
pub const VISIBILITY_PENDING_SWAPS: u32 = 2;

/// Visibility-test indices: 64 report pages of 256 slots
/// (`0x3fc69d..0x3fc6bd` frees 0x40 pages).
pub const VISIBILITY_INDICES: u32 = 0x40 * 0x100;

/// One ended visibility test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VisibilityTest {
    /// GPU time (`+0x2C`) when it ended: the original stamps the slot with
    /// it, and the device reports it as the timestamp.
    pub time: u32,
    /// The device's swap count when it ended.
    pub swap: u32,
    /// The count.
    pub result: VisibilityResult,
}

/// Where a test's count is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VisibilityResult {
    /// Still on the backend: its pixel-count ticket.
    Ticket(u64),
    /// Read.
    Count(u32),
}

/// What [`Device::get_visibility_test_result`] answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VisibilityAnswer {
    /// Samples that passed (saturated to 32 bits, as the report word is).
    pub count: u32,
    /// Timestamp written through `EDX` when non-null.
    pub timestamp: u64,
}

impl<B: Backend> Device<B> {
    /// `NV097_CLEAR_REPORT_VALUE`, from the drain: restarts the backend's
    /// sample count. Only type 1 (`ZPASS_PIXEL_CNT`) exists.
    pub(super) fn clear_report(&mut self, at: u32, kind: u32) -> Result<(), D3dError> {
        if kind != 1 {
            return Err(D3dError::unimplemented(
                "push buffer",
                format!("CLEAR_REPORT_VALUE type {kind} at {at:#010x} (only 1, ZPASS_PIXEL_CNT)"),
            ));
        }
        self.backend.clear_pixel_count().map_err(D3dError::Backend)
    }

    fn check_index(entry: &'static str, index: u32) -> Result<(), D3dError> {
        if index >= VISIBILITY_INDICES {
            return Err(D3dError::invalid(
                entry,
                format!(
                    "index {index:#x}: the original indexes 64 report pages of 256 slots (below {VISIBILITY_INDICES:#x})"
                ),
            ));
        }
        Ok(())
    }

    /// `D3DDevice_EndVisibilityTest` (`0x3f85d0`, `EAX` = index; `ret`;
    /// returns `EAX` = 0). Drains the queue (so the draws since the inline
    /// begin have run), turns counting off as the original's
    /// `SET_ZPASS_PIXEL_COUNT_ENABLE` 0 does, and takes the backend's report
    /// for the slot. Ending an index again replaces its result, as a new
    /// report overwrites the slot.
    pub fn end_visibility_test(
        &mut self,
        mem: &mut dyn GuestMemory,
        index: u32,
    ) -> Result<(), D3dError> {
        const E: &str = "D3DDevice_EndVisibilityTest";
        self.live(E)?;
        Self::check_index(E, index)?;
        self.sync_residual_entry(mem, EntryKind::EndQuery)?;
        let time = mem.read_u32(self.cfg.layout.device + field::GPU_TIME)?;
        let ticket = self
            .backend
            .report_pixel_count()
            .map_err(D3dError::Backend)?
            .ok_or_else(|| {
                D3dError::unimplemented(
                    E,
                    "the backend cannot count pixels (Backend::report_pixel_count)",
                )
            })?;
        self.hw.zpass_count_enable = false;
        let old = self.visibility.insert(
            index,
            VisibilityTest {
                time,
                swap: self.swap_count,
                result: VisibilityResult::Ticket(ticket),
            },
        );
        if let Some(VisibilityTest {
            result: VisibilityResult::Ticket(t),
            ..
        }) = old
        {
            self.backend.drop_pixel_count(t);
        }
        self.stats.visibility_tests += 1;
        Ok(())
    }

    /// `D3DDevice_GetVisibilityTestResult` (`0x3f8630`). `None` is
    /// `D3DERR_TESTINCOMPLETE`: polling, a recent test whose count is not
    /// back (see the module docs). A count once answered is kept, so asking
    /// again answers the same, as the slot would. An index that was never
    /// ended stops: the original would read a slot of a page that may not
    /// exist.
    pub fn get_visibility_test_result(
        &mut self,
        mem: &mut dyn GuestMemory,
        index: u32,
    ) -> Result<Option<VisibilityAnswer>, D3dError> {
        const E: &str = "D3DDevice_GetVisibilityTestResult";
        self.live(E)?;
        Self::check_index(E, index)?;
        self.sync_residual_entry(mem, EntryKind::QueryResult)?;
        let Some(test) = self.visibility.get(&index).copied() else {
            return Err(D3dError::invalid(
                E,
                format!("visibility test {index:#x} was never ended (EndVisibilityTest)"),
            ));
        };
        let count = match test.result {
            VisibilityResult::Count(c) => c,
            VisibilityResult::Ticket(t) => {
                let recent = self.swap_count.wrapping_sub(test.swap) <= VISIBILITY_PENDING_SWAPS;
                let back = if self.visibility_poll && recent {
                    self.backend.try_pixel_count(t)
                } else {
                    self.backend.read_pixel_count(t).map(Some)
                };
                let Some(samples) = back.map_err(D3dError::Backend)? else {
                    // 0x3f8657..0x3f8660: a fence when nothing has moved
                    // since the end, so the work reaches the GPU.
                    let dev = self.cfg.layout.device;
                    if mem.read_u32(dev + field::GPU_TIME)? == test.time {
                        self.set_fence(mem)?;
                    }
                    self.stats.visibility_incomplete += 1;
                    return Ok(None);
                };
                let c = u32::try_from(samples).unwrap_or(u32::MAX);
                self.visibility.insert(
                    index,
                    VisibilityTest {
                        result: VisibilityResult::Count(c),
                        ..test
                    },
                );
                c
            }
        };
        Ok(Some(VisibilityAnswer {
            count,
            timestamp: test.time as u64,
        }))
    }

    /// Whether `GetVisibilityTestResult` polls (on by default;
    /// `H2_D3D_VISIBILITY_POLL=0` turns it off): a recent test whose count
    /// is not back answers `D3DERR_TESTINCOMPLETE` instead of waiting for
    /// it (see the module docs). Off, every call waits, and the answers do
    /// not depend on host timing.
    pub fn set_visibility_poll(&mut self, on: bool) {
        self.visibility_poll = on;
    }

    /// Forgets every visibility test (teardown frees the report pages).
    pub(super) fn drop_visibility_tests(&mut self) {
        for (_, t) in self.visibility.drain() {
            if let VisibilityResult::Ticket(ticket) = t.result {
                self.backend.drop_pixel_count(ticket);
            }
        }
    }
}
