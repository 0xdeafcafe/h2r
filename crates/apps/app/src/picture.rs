//! `render.picture`'s frame source (docs/runtime/agent-api.md, "Picture
//! feed"): the newest frame of the handoff, read back on the asking thread
//! (the agent's picture worker), attached at [`agent_proto::picture::hub`]
//! for as long as the window or the headless host lives.

use std::sync::{Arc, Mutex};

use agent_proto::picture::Grabbed;
use gfx::GfxContext;

use crate::frame::FrameSource;

/// What the readback needs, once the handoff is up (the window fills it
/// in at init; a headless host has it at once).
pub(crate) type Parts = Arc<Mutex<Option<(GfxContext, FrameSource)>>>;

/// Detaches the source when dropped.
pub(crate) struct Attached;

impl Drop for Attached {
    fn drop(&mut self) {
        agent_proto::picture::hub().detach();
    }
}

/// Attaches `parts` as the process's picture source.
pub(crate) fn attach(parts: Parts) -> Attached {
    agent_proto::picture::hub().attach(Box::new(move |after| grab(&parts, after)));
    Attached
}

/// The newest published frame (or the loading picture) unless it is
/// `after`. A source still starting, or with no frame yet, has none.
fn grab(parts: &Parts, after: Option<u64>) -> Result<Option<Grabbed>, String> {
    let Some((ctx, source)) = parts.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return Ok(None);
    };
    let Some(frame) = source.peek() else {
        return Ok(None);
    };
    if after == Some(frame.seq) {
        return Ok(None);
    }
    let rgba = gfx::readback::try_read_texture(
        &ctx,
        &frame.texture,
        wgpu::TextureAspect::All,
        4,
        frame.size,
    )?;
    Ok(Some(Grabbed {
        seq: frame.seq,
        size: frame.size,
        rgba,
    }))
}
