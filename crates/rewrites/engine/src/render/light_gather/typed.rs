//! The constants, flags, light rectangle and shadow projection for a complete
//! set of typed passes. The caller supplies this light's posed volume and
//! the shadow argument after the caster mask test. Planning touches no
//! guest state. Publish the setup only after the graph takes the whole light.

use crate::light_constants::{self as lc, Error, Fpu, LightGlobals, Sse};
use crate::light_passes::{self as lp, PassFlags};
use guest_layout::memory::{GuestMemory, MemFault};
use crate::shadow_constants::{self as sc, ShadowGlobals};

/// Which caller supplied the `0x0004a1e0` arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    Dynamic,
    FirstPerson,
    /// The mask's pass flags and the falloff word stored *after* its
    /// projection has been computed (0x0003290a).
    ObjectShadow {
        flags: u32,
        weight: u32,
    },
}

/// The five shader-input masks that `0x0001be50` marks dirty at the
/// start of `0x0003a550`, before the shader constants upload.
const DIRTY: u32 = 0x0048_5b78;
const INPUT_MASKS: [u32; 5] = [
    0x0048_5084,
    0x0048_5088,
    0x0048_50b4,
    0x0048_50b8,
    0x0048_5064,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Globals {
    pub light: LightGlobals,
    pub flags: u32,
    pub shadow: Option<ShadowGlobals>,
    pass_flags: PassFlags,
    site: Site,
    dirty: [u32; 5],
}

fn pass_flags(
    m: &dyn GuestMemory,
    light: &LightGlobals,
    mxcsr: u32,
    site: Site,
) -> Result<PassFlags, Error> {
    let flags = match site {
        Site::Dynamic => return lp::pass_flags(m, light, light.shadow, mxcsr),
        Site::FirstPerson => {
            // 0x0004059f..0x000405c6: ja on each component, stopping at
            // the first one above zero. This site keeps both colours.
            let mut sse = Sse { mxcsr };
            let mut flags = u32::from(light.context_word == 0);
            for c in light.colour_b {
                if sse.above(c, 0)? {
                    flags |= lp::flag::SPECULAR;
                    break;
                }
            }
            flags
        }
        Site::ObjectShadow { flags, .. } => flags,
    };
    Ok(PassFlags {
        flags,
        colour_b: light.colour_b,
        colour_b_zeroed: false,
    })
}

/// Reads every input and output range of the setup, including the native
/// bounds answer. A failed projection is distinct from an offscreen answer:
/// only the latter uses the guest's full viewport fallback.
pub fn plan(m: &dyn GuestMemory, call: &lc::Call, fpu: Fpu, site: Site) -> Result<Globals, Error> {
    let light = lc::projected_light_globals(m, call, fpu)?;
    let pass_flags = pass_flags(m, &light, fpu.mxcsr, site)?;
    let shadow = if pass_flags.shadowed() || matches!(site, Site::ObjectShadow { .. }) {
        // 0x0001cf50 compares the four desired textures (all cleared by
        // shadow setup) against their cached bindings. A live binding
        // needs a real SetTexture and its reference-count changes.
        for stage in 0..4 {
            if m.u32(sc::global::TEXTURES - 16 + stage * 4)? != 0 {
                return Err(Error::BoundTexture);
            }
        }
        let projection = sc::Call {
            context: call.context,
            frame: call.frame,
            definition: call.definition,
        };
        Some(sc::shadow_globals_for_light(
            m,
            &projection,
            fpu,
            &light,
            pass_flags.flags,
        )?)
    } else {
        None
    };
    let mut dirty = [0; 5];
    for (i, (word, mask)) in dirty.iter_mut().zip(INPUT_MASKS).enumerate() {
        *word = m.u32(DIRTY + 4 * i as u32)? | m.u32(mask)?;
    }
    let out = Globals {
        light,
        flags: pass_flags.flags,
        shadow,
        pass_flags,
        site,
        dirty,
    };
    out.write(&mut CheckRanges(m))?;
    Ok(out)
}

// All published globals are in guest RAM. Check the actual byte ranges
// of the writers before admission, without touching the backing memory.
struct CheckRanges<'a>(&'a dyn GuestMemory);

impl GuestMemory for CheckRanges<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), MemFault> {
        self.0.read(addr, out)
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        let mut scratch = [0; sc::BLOCK_WORDS * 4];
        if let Some(bytes) = scratch.get_mut(..data.len()) {
            self.0.read(addr, bytes)
        } else {
            self.0.bytes(addr, data.len()).map(|_| ())
        }
    }
}

impl Globals {
    /// The object mask sets its flags before constants. Its weight
    /// overrides the first falloff word after the shadow setup. The
    /// dynamic and first-person sites set flags after light constants.
    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        if matches!(self.site, Site::ObjectShadow { .. }) {
            self.pass_flags.write(m)?;
        }
        self.light.write(m)?;
        for (i, mask) in self.dirty.iter().enumerate() {
            m.set_u32(DIRTY + 4 * i as u32, *mask)?;
        }
        if !matches!(self.site, Site::ObjectShadow { .. }) {
            self.pass_flags.write(m)?;
        }
        if let Some(shadow) = &self.shadow {
            shadow.write(m)?;
        }
        if let Site::ObjectShadow { weight, .. } = self.site {
            m.set_u32(lc::global::FALLOFF, weight)?;
        }
        Ok(())
    }
}
