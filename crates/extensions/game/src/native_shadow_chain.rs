//! Private object-shadow chains (docs/graphics/native-shadow-layers.md,
//! "A private chain"). When an object's layer-10 group draws alone, its
//! casters go into a `native_render::shadow_chain::Pair` instead of the
//! guest's target 10, the filter runs over that pair, and the receivers
//! sample its mask. The guest's begin still clears target 10; nothing else
//! of the chain writes targets 10 or 11, and the freeze does not run.
//!
//! A filter the stage does not take hands the casters back: [`Spill`]
//! copies the private mask into target 10 before the guest's quads, and
//! from there the chain is the guest's (freeze included).

use crate::native_shadows_draw::Output;
use native_render::shadow_chain::Pair;
use std::cell::RefCell;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// One generation's private chain, shared by its caster pass, its filter
/// pass and the receiver boundary.
pub struct Chain {
    ctx: gfx::GfxContext,
    /// One pair per target copy (a scaled backend records two sizes).
    pairs: Mutex<Vec<Arc<Pair>>>,
    spilled: AtomicBool,
    output: Output,
}

impl Chain {
    /// `output` is the layer-10 call's: the receiver term of layer 11
    /// resolves through it.
    pub fn new(ctx: gfx::GfxContext, output: Output) -> Arc<Self> {
        Arc::new(Self {
            ctx,
            pairs: Mutex::new(Vec::new()),
            spilled: AtomicBool::new(false),
            output,
        })
    }

    /// Whether the chain went back to the guest's targets.
    pub fn spilled(&self) -> bool {
        self.spilled.load(Ordering::Acquire)
    }

    /// Mark the chain spilled; `true` only for the call that did.
    pub fn spill_once(&self) -> bool {
        !self.spilled.swap(true, Ordering::AcqRel)
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

/// The caster pass recorded into the chain's pair. The inner pass's
/// packets are recorded for the bound target 10's formats and size, the
/// pair's own.
pub struct PrivateCasters {
    inner: Box<dyn d3d::ExternalPass>,
    chain: Arc<Chain>,
}

impl PrivateCasters {
    pub fn new(inner: Box<dyn d3d::ExternalPass>, chain: Arc<Chain>) -> Self {
        Self { inner, chain }
    }

    fn encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Result<d3d::backend::ExternalEncoded, &'static str> {
        let draws = self
            .inner
            .record_draws(t, request)
            .unwrap_or(Err("caster pass has no packets"))?;
        if draws.packets.iter().any(gfx::RecordedDraw::counts_samples) {
            return Err("caster writes a visibility query");
        }
        let pair = Arc::new(
            Pair::scaled(
                &t.device,
                t.color_format,
                Some(t.depth_format),
                t.size,
                request.surface_size,
            )
            .ok_or("private shadow pair unavailable")?,
        );
        // The packets' persistent bytes go ahead of the commands, as the
        // backend writes them for a packet run.
        for (buffer, offset, bytes) in &draws.writes {
            t.queue.write_buffer(buffer, *offset, bytes);
        }
        self.chain.add(&pair)?;
        let ctx = self.chain.ctx.clone();
        let packets = draws.packets;
        Ok(d3d::backend::ExternalEncoded {
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

impl d3d::ExternalPass for PrivateCasters {
    fn uses_validated_resources(&self) -> bool {
        true
    }

    fn colour_only(&self) -> bool {
        true
    }

    /// Nothing bound is written: the pair is the stage's own.
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites::default()
    }

    fn record_encoded(
        &mut self,
        t: &d3d::backend::ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<d3d::backend::ExternalEncoded, &'static str>> {
        let result = self.encoded(t, request);
        if let Err(why) = result {
            render_bridge::native_shadows::dropped_layer();
            eprintln!("native-shadows: a private caster pass failed while recording: {why}");
        }
        Some(result)
    }

    fn view_constants(&mut self, constants: &[[f32; 4]; d3d::VIEW_CONSTANTS]) {
        self.inner.view_constants(constants);
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.inner.depth_clip(clip);
    }
}

/// Copy a private mask into the guest's target 10 (`header`), so the
/// guest's filter and the freeze after it see the casters.
pub struct Spill {
    pub chain: Arc<Chain>,
    pub header: u32,
}

impl d3d::ExternalPass for Spill {
    fn colour_only(&self) -> bool {
        true
    }

    fn sampled(&self) -> Vec<u32> {
        vec![self.header]
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let [Some(level)] = t.sampled else {
            render_bridge::native_shadows::dropped_layer();
            eprintln!("native-shadows: a spilled chain has no target 10 to copy into");
            return;
        };
        let dest = level.view.texture();
        match self.chain.pair_for(t.size) {
            Some(pair) if pair.spills_into(dest) => {
                pair.spill(t.encoder, dest);
            }
            _ => {
                render_bridge::native_shadows::dropped_layer();
                eprintln!("native-shadows: a spilled chain's mask does not fit target 10");
            }
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
    fn a_chain_spills_once_and_publishes_its_mask_into_the_shared_output() {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let output: Output = Default::default();
        let chain = Chain::new(ctx.clone(), Arc::clone(&output));
        assert!(!chain.spilled());
        assert!(chain.pair().is_none());
        let pair = Pair::new(&ctx.device, wgpu::TextureFormat::Rgba8Unorm, None, (8, 4)).unwrap();
        chain.publish(&pair);
        let map = output.lock().unwrap().latest().cloned().unwrap();
        assert_eq!(map.size, (8, 4));
        assert_eq!(map.view.texture(), pair.mask.texture());
        assert!(chain.spill_once());
        assert!(!chain.spill_once());
        assert!(chain.spilled());
    }
}
