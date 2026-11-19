//! A producer boundary drain that leaves interrupt callbacks queued.
use super::*;
impl<B: Backend> Device<B> {
    /// Drain only already-published commands. False means an interrupt
    /// callback must run first; it stays queued for the ordinary D3D entry.
    /// A caller must cancel its admission scope on false or error.
    pub fn drain_admission_boundary(
        &mut self,
        mem: &mut dyn GuestMemory,
    ) -> Result<bool, D3dError> {
        if let Some(cb) = self.drain_until_callback(mem)? {
            self.undelivered = Some(cb);
            Ok(false)
        } else {
            Ok(true)
        }
    }
}
