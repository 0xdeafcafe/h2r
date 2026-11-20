//! A parity reply is resolved by the renderer. Polling it at tick boundaries
//! must leave the game free to seal the scene or submit the reference frame.

use std::sync::{Arc, Weak, mpsc};

use agent_proto::{ParityReply, RpcError, code};

use crate::exec::{DEFER_TICKS_MAX, GameCtx};

/// Installs a one-frame comparison; the request carries its own reply channel.
pub type ParitySource = fn(ParityRequest);

/// The host's answer to one arming of `render.parity`.
pub struct ParityRequest {
    waiting: Weak<()>,
    result: mpsc::Sender<Result<ParityReply, RpcError>>,
}

impl ParityRequest {
    /// A timed-out request can be discarded before either render or readback.
    pub fn waiting(&self) -> bool {
        self.waiting.strong_count() != 0
    }

    pub fn finish(self, result: Result<ParityReply, RpcError>) {
        let _ = self.result.send(result);
    }
}

pub(super) fn arm(ctx: &mut GameCtx<'_>, source: ParitySource) {
    let waiting = Arc::new(());
    let (tx, rx) = mpsc::channel();
    source(ParityRequest { waiting: Arc::downgrade(&waiting), result: tx });
    ctx.defer(DEFER_TICKS_MAX, Box::new(move |_| {
        let _keep_request = &waiting;
        match rx.try_recv() {
            Ok(r) => Some(r.and_then(|reply| serde_json::to_value(reply)
                .map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string())))),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(RpcError::new(
                code::UNAVAILABLE, "the renderer stopped before the parity frame was compared",
            ))),
        }
    }));
}
