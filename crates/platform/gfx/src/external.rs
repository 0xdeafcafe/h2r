//! What a native stage hands the D3D backend to join the frame
//! (docs/graphics/framescene.md, section 4.1): the target copy a
//! recording is for, prepared draw packets and encoded commands. Both
//! sides need them, so they live here; `d3d::backend` re-exports them at
//! their old paths, and `native-render`'s world and object stages build
//! them without depending on `d3d`.

/// Which physical copy of a guest target a recording draws into: the 1x
/// reference the guest can read back, or the display copy at the render
/// scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetCopy {
    Reference,
    Display,
}

/// Draws prepared with cached pipelines and bindings for an ordered run
/// of a stage on its attachments: appended to the render pass already
/// open on them, without an encoder of their own.
#[cfg(feature = "gpu")]
pub struct ExternalDraws {
    /// Guest ranges represented by the packets (a range may have several
    /// scissors, and adjacent ranges may share a packet).
    pub ranges: usize,
    pub packets: Vec<crate::RecordedDraw>,
    pub discards: bool,
    /// Persistent buffer ranges the packets read, written ahead of them:
    /// at the front of the submission when it holds no other bytes for
    /// them, otherwise in draw order before the run (`d3d`'s `GfxBackend`).
    pub writes: Vec<(wgpu::Buffer, u64, Vec<u8>)>,
}

/// Commands a pass issues into the frame's open encoder at commit.
/// Everything they use is made and written at prepare; the closure only
/// records, once on a scratch encoder under a validation scope and once
/// for real.
#[cfg(feature = "gpu")]
pub struct ExternalEncoded {
    /// Guest ranges recorded.
    pub ranges: usize,
    /// GPU draws the commands issue, including any intermediate draws.
    pub draws: usize,
    /// Render passes the commands begin (a submission's passes are bounded).
    pub passes: usize,
    pub commands: Box<dyn Fn(&mut wgpu::CommandEncoder) + Send>,
}
