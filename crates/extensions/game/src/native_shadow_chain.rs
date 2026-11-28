//! Private object-shadow chains (docs/graphics/native-shadow-layers.md,
//! "A private chain"). When an object's layer-10 group draws alone, its
//! casters go into a `native_render::shadow_chain::Pair` instead of the
//! object's target 10, the filter runs over that pair, and the receivers
//! sample its mask.

use crate::native_shadows_draw::Output;
use gfx::external::{ExternalAttachments, ExternalEncoded};
use native_render::shadow_chain::Pair;
use native_render::program::ViewInputs;
use native_render::stage::{Stage, StageWork};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

/// One generation's private chain, shared by its caster pass, its filter
/// pass and the receiver boundary.
pub struct Chain {
    ctx: gfx::GfxContext,
    /// The pairs by size: one per render scale the chain recorded at.
    pairs: Mutex<Vec<Arc<Pair>>>,
    output: Output,
}

impl Chain {
    /// `output` is the layer-10 call's: the receiver term of layer 11
    /// resolves through it.
    pub fn new(ctx: gfx::GfxContext, output: Output) -> Arc<Self> {
        Arc::new(Self {
            ctx,
            pairs: Mutex::new(Vec::new()),
            output,
        })
    }

    /// The last pair the caster pass made when it recorded.
    pub fn pair(&self) -> Option<Arc<Pair>> {
        self.pairs.lock().ok()?.last().cloned()
    }

    /// The pair of the target copy whose texels are `size`.
    pub fn pair_for(&self, size: (u32, u32)) -> Option<Arc<Pair>> {
        self.pairs.lock().ok()?.iter().find(|p| p.size == size).cloned()
    }

    fn add(&self, pair: &Arc<Pair>) -> Result<(), &'static str> {
        let mut pairs = self.pairs.lock().map_err(|_| "private shadow pair poisoned")?;
        pairs.retain(|p| p.size != pair.size);
        pairs.push(Arc::clone(pair));
        Ok(())
    }

    /// The filter's commands are recorded: the receivers may sample the
    /// mask from here on in GPU order.
    pub fn publish(&self, pair: &Pair) {
        self.output
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .put(pair.size, Some(pair.sampled()));
    }
}

thread_local! {
    /// The chain offered to the layer-10 draw now running on this thread.
    static OFFER: RefCell<Option<Arc<Chain>>> = const { RefCell::new(None) };
}

/// Offer `chain` to the caster pass the current `draw_alone` builds.
pub(crate) fn offer(chain: Arc<Chain>) {
    OFFER.set(Some(chain));
}

/// Claim the offer, just before the caster pass is reserved.
pub(crate) fn take() -> Option<Arc<Chain>> {
    OFFER.take()
}

/// After the draw: the offer if nobody claimed it.
pub(crate) fn withdraw() -> Option<Arc<Chain>> {
    OFFER.take()
}

/// The caster pass recorded into the chain's pair. The inner stage's
/// packets are recorded for the bound attachments' formats and size, the
/// pair's own.
pub struct PrivateCasters {
    inner: Box<dyn Stage>,
    chain: Arc<Chain>,
}

impl PrivateCasters {
    pub fn new(inner: Box<dyn Stage>, chain: Arc<Chain>) -> Self {
        Self { inner, chain }
    }

    fn encoded(&mut self, t: &ExternalAttachments, view: &ViewInputs) -> Result<ExternalEncoded, &'static str> {
        let StageWork::Draws(draws) = self.inner.record(t, view) else {
            return Err("caster pass has no packets");
        };
        if draws.packets.iter().any(gfx::RecordedDraw::counts_samples) {
            return Err("caster writes a visibility query");
        }
        let pair = Arc::new(
            Pair::scaled(&t.device, t.color_format, Some(t.depth_format), t.size, view.guest_size)
                .ok_or("private shadow pair unavailable")?,
        );
        // The packets' persistent bytes go ahead of the commands.
        for (buffer, offset, bytes) in &draws.writes {
            t.queue.write_buffer(buffer, *offset, bytes);
        }
        self.chain.add(&pair)?;
        let ctx = self.chain.ctx.clone();
        let packets = draws.packets;
        Ok(ExternalEncoded {
            ranges: draws.ranges,
            draws: packets.len(),
            passes: 1,
            commands: Box::new(move |encoder| {
                // Checked above; encode_pass refuses nothing else.
                let _ = pair.record_casters(&ctx, encoder, &packets);
            }),
        })
    }
}

impl Stage for PrivateCasters {
    fn name(&self) -> &'static str {
        "shadow_private_casters"
    }

    fn record(&mut self, t: &ExternalAttachments, view: &ViewInputs) -> StageWork {
        match self.encoded(t, view) {
            Ok(encoded) => StageWork::Encoded(encoded),
            Err(why) => panic!("shadows: private caster pass: {why}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_offer_is_claimed_once_and_an_unclaimed_one_comes_back() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let chain = Chain::new(ctx, Default::default());
        offer(Arc::clone(&chain));
        assert!(take().is_some_and(|c| Arc::ptr_eq(&c, &chain)));
        assert!(withdraw().is_none(), "a claimed offer is not returned");
        offer(Arc::clone(&chain));
        assert!(withdraw().is_some(), "an unclaimed offer is returned");
        assert!(take().is_none());
    }

    #[test]
    fn a_scaled_chain_holds_a_pair_and_a_mask_per_copy() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let output: Output = Default::default();
        let chain = Chain::new(ctx.clone(), Arc::clone(&output));
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let guest = (8, 4);
        for size in [guest, (16, 8), (32, 16)] {
            let pair = Arc::new(Pair::scaled(&ctx.device, format, None, size, guest).unwrap());
            assert_eq!(pair.filter_slots()[0].guest_size, guest);
            chain.add(&pair).unwrap();
            chain.publish(&pair);
        }
        for size in [guest, (16, 8), (32, 16)] {
            assert_eq!(chain.pair_for(size).unwrap().size, size);
            assert_eq!(output.lock().unwrap().at(size).unwrap().size, size);
        }
        assert!(chain.pair_for((64, 32)).is_none());
        assert_eq!(chain.pair().unwrap().size, (32, 16));
    }

    #[test]
    fn a_chain_publishes_its_mask_into_the_shared_output() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let output: Output = Default::default();
        let chain = Chain::new(ctx.clone(), Arc::clone(&output));
        assert!(chain.pair().is_none());
        let pair = Pair::new(&ctx.device, wgpu::TextureFormat::Rgba8Unorm, None, (8, 4)).unwrap();
        chain.publish(&pair);
        let map = output.lock().unwrap().latest().cloned().unwrap();
        assert_eq!(map.size, (8, 4));
        assert_eq!(map.view.texture(), pair.mask.texture());
    }
}
