//! Native attachment and visibility operations after the resolved entry
//! drain. The original device owns surface identity and query tickets.

use super::*;
use crate::abi::{self, Entry, Outcome, Regs};
use crate::backend::ChannelMask;
use gfx::Clear;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EntryKind {
    Clear,
    Bind,
    EndQuery,
    QueryResult,
}

impl EntryKind {
    fn at(addr: u32) -> Option<Self> {
        match addr {
            0x003f_9fc0 => Some(Self::Clear),
            0x003f_7870 | 0x003f_7b30 => Some(Self::Bind),
            0x003f_85d0 => Some(Self::EndQuery),
            0x003f_8630 => Some(Self::QueryResult),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(super) struct Residual {
    pub(super) enabled: bool,
    drained: Option<EntryKind>,
}

impl<B: Backend> Device<B> {
    pub fn set_native_residual(&mut self, enabled: bool) {
        self.residual = Residual {
            enabled,
            drained: None,
        };
    }

    /// Only runtime glue can supply this boundary. It already holds the
    /// memory and device locks and has delivered any earlier callback.
    pub(crate) fn call_residual_entry(
        &mut self,
        entry: &Entry<B>,
        mem: &mut dyn GuestMemory,
        regs: &Regs,
    ) -> Result<Outcome, D3dError> {
        let Some(kind) = self
            .residual
            .enabled
            .then_some(entry.addr)
            .and_then(EntryKind::at)
        else {
            return abi::call(entry, self, mem, regs);
        };
        self.residual.drained = Some(kind);
        let result = abi::call(entry, self, mem, regs);
        // Also clear it if argument decoding or live-device checks failed
        // before the entry consumed its drain.
        self.residual.drained = None;
        result
    }

    pub(super) fn sync_residual_entry(
        &mut self,
        mem: &mut dyn GuestMemory,
        kind: EntryKind,
    ) -> Result<(), D3dError> {
        if self.residual.drained.take() == Some(kind) {
            return Ok(());
        }
        self.sync_push_buffer(mem)
    }

    /// Whole attachments need neither a rectangle list nor a clear shader.
    /// The caller checked that the viewport covers the target, count is
    /// zero, and color clears select either all channels or none.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn native_attachment_clear(
        &mut self,
        mem: &mut dyn GuestMemory,
        entry: &'static str,
        flags: u32,
        mask: ChannelMask,
        color: u32,
        z: f32,
        stencil: u32,
    ) -> Result<(), D3dError> {
        let (target, depth) = self.current_targets(mem, entry)?;
        let clear = Clear {
            color: mask.all().then(|| d3dcolor_to_rgba(color)),
            depth: (depth.is_some() && flags & clear_flags::ZBUFFER != 0).then_some(z),
            stencil: (depth.is_some() && flags & clear_flags::STENCIL != 0)
                .then_some(stencil as u8),
        };
        if clear.color.is_none() && clear.depth.is_none() && clear.stencil.is_none() {
            return Ok(());
        }
        self.before_target_write(target)?;
        self.backend
            .clear(target, depth, clear)
            .map_err(D3dError::Backend)?;
        self.mark_written(clear.depth.is_some() || clear.stencil.is_some());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_boundary_is_only_for_attachment_and_visibility_entries() {
        assert_eq!(EntryKind::at(0x003f_9fc0), Some(EntryKind::Clear));
        assert_eq!(EntryKind::at(0x003f_7870), Some(EntryKind::Bind));
        assert_eq!(EntryKind::at(0x003f_7b30), Some(EntryKind::Bind));
        assert_eq!(EntryKind::at(0x003f_85d0), Some(EntryKind::EndQuery));
        assert_eq!(EntryKind::at(0x003f_8630), Some(EntryKind::QueryResult));
        assert_eq!(EntryKind::at(0x003f_afa0), None);
    }
}
