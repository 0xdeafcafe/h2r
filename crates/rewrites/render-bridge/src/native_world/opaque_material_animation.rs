//! Shader instance animations at the ordinary material bind. The compiled
//! LOD tree selects five independent ranges. The bitmap range runs before
//! resolving the four texture stages. The remaining ranges run after the
//! fixed bindings, in pixel, vertex, render-state, sampler-state order.

use std::ops::Range;

use hooks::{Cx, GuestCall};
use guest_layout::entry::EntryError;
use engine::light_constants::Error;
use guest_layout::memory::{GuestMemory, MemFault};
use engine::render_loop::{opaque_animation as evaluate, states};
use super::{opaque_material_registers, opaque_material_states};

const INPUT_SOURCE: u32 = 0x0048_5a58;
const PROPERTY_EVALUATOR: u32 = 0x0001_b230;
const STAGE_DEFS: u32 = 0x0046_703c;
const STAGE_SHADOW: u32 = 0x0040_4dd0;
const TEXTURE_SLOTS: u32 = 0x320;
const VECTOR_BANK: u32 = 0x1530;
const VECTOR_DIRTY: u32 = 0x1730;

fn error(e: Error) -> EntryError {
    match e {
        Error::Memory(f) => EntryError::Fault(f),
        _ => EntryError::IsolatedCall(format!("shader animation: {e:?}")),
    }
}

fn span(word: u16) -> Range<u32> {
    let start = u32::from(word & 511);
    start..start+u32::from(word >> 9)
}

/// Immutable animation sources shared by the entries using the same
/// selected shader in this consecutive sorted span. Only the evaluated
/// values, callbacks and destination caches depend on the previous draw.
#[derive(Clone, Copy)]
pub(super) struct Leaf {
    bindings: u32,
    properties: u32,
    references: u32,
    ranges: u32,
    targets: [u16; 5],
}

impl Leaf {
    fn read(m: &dyn GuestMemory, instance: u32, leaf: u32) -> Result<Option<Self>, MemFault> {
        let mut targets = [0; 5];
        for (i, target) in targets.iter_mut().enumerate() {
            *target = m.u16(leaf + i as u32 * 2)?;
        }
        if targets.iter().all(|word| word >> 9 == 0) { return Ok(None); }
        Ok(Some(Self {
            targets,
            bindings: m.u32(instance + 0x58)?,
            ranges: m.u32(instance + 0x50)?,
            references: m.u32(instance + 0x48)?,
            properties: m.u32(instance + 0x40)?,
        }))
    }

    fn bindings(self, kind: usize) -> impl Iterator<Item = u32> {
        span(self.targets[kind]).map(move |n| self.bindings + n * 4)
    }
}

pub(super) fn prepare(m: &dyn GuestMemory, instance: u32, lod: u32, layer: u32,
    pass: u32, implementation: u32) -> Result<Option<Leaf>, MemFault> {
    if lod as i32 >= m.i32(instance + 0x1c)? { return Ok(None); }
    let row = m.u32(instance + 0x20)?.wrapping_add(lod.wrapping_mul(6));
    if m.u32(row)? & 1u32.wrapping_shl(layer) == 0 { return Ok(None); }
    let mut range = span(m.u16(row + 4)?);
    for (selector, pointer) in [(layer, 0x28), (pass, 0x30)] {
        if selector >= range.len() as u32 { return Ok(None); }
        let base = m.u32(instance + pointer)?;
        range = span(m.u16(base + (range.start + selector) * 2)?);
    }
    if implementation >= range.len() as u32 { return Ok(None); }
    let leaf = m.u32(instance + 0x38)? + (range.start + implementation) * 10;
    Leaf::read(m, instance, leaf)
}

struct PropertyRefs {
    references: u32,
    properties: u32,
    range: Range<u32>,
}

impl PropertyRefs {
    fn read(m: &dyn GuestMemory, leaf: Leaf, binding: u32) -> Result<Self, MemFault> {
        let range = span(m.u16(leaf.ranges + u32::from(m.u8(binding + 3)?) * 2)?);
        Ok(Self { references: leaf.references, properties: leaf.properties, range })
    }

    fn reference(&self,m:&dyn GuestMemory,i:u32) -> Result<(u32,u32),MemFault> {
        let slot = self.references+i*4;
        let index = i32::from(m.u16(slot)? as i16);
        Ok((self.properties.wrapping_add_signed(index*20),slot))
    }
}

async fn value(cx:&Cx, property:u32, fcw:u16, mxcsr:u32) -> Result<u32,EntryError> {
    // The input callback can sample an object, and can have effects even
    // when a time property overwrites its first result. Enter the original
    // evaluator for that case to preserve both x87 callback returns.
    let callback = cx.read(|m| -> Result<_,MemFault> {
        let driver = m.u32(INPUT_SOURCE)?;
        Ok(driver != 0 && m.u32(driver+4)? != 0)
    })?;
    if callback {
        let call = cx.read(|m| -> Result<_,MemFault> {
            Ok(GuestCall::new(PROPERTY_EVALUATOR).eax(property+12).esi(m.u32(property)?)
                .args([m.u32(property+4)?,m.u32(property+8)?]))
        })?;
        return Ok(cx.call(call).await?.xmm0 as u32);
    }
    cx.read(|m| evaluate::evaluate(m,property,fcw,mxcsr)).map_err(error)
}

/// All references attached to a texture target are applied (other binding
/// types use just the first reference). Offsets are signed component indices.
pub(super) async fn textures(cx:&Cx,cache:u32,leaf:Leaf,fcw:u16,mxcsr:u32) -> Result<(),EntryError> {
    for at in leaf.bindings(0) {
        let refs = cx.read(|m| PropertyRefs::read(m,leaf,at))?;
        for i in refs.range.clone() {
            let (property,slot) = cx.read(|m| refs.reference(m,i))?;
            let v = value(cx,property,fcw,mxcsr).await?;
            cx.write(|m| -> Result<(),Error> {
                let v = evaluate::ranged(m,property+12,v,mxcsr)?;
                let texture = u32::from(m.u8(at)?);
                let component = i32::from(m.u16(slot+2)? as i16);
                let dest = (cache+TEXTURE_SLOTS+0x18+texture*64).wrapping_add_signed(component*4);
                Ok(m.set_u32(dest,v)?)
            }).map_err(error)?;
        }
    }
    Ok(())
}

#[derive(Clone,Copy)]
enum Target { Pixel, Vertex, Render, Stage }

fn state_value(m:&dyn GuestMemory, reference:u32, raw:u32, scalar:bool, def:u32, shadow:u32, fcw:u16,mxcsr:u32)
    -> Result<Option<u32>,Error> {
    let rgb_mask = 0x00ff_ffff;
    if !scalar {
        let rgb = evaluate::colour(m,reference,raw,fcw,mxcsr)?;
        return Ok(Some((m.u32(shadow)? & !rgb_mask) | (rgb & rgb_mask)));
    }
    let v = evaluate::ranged(m,reference,raw,mxcsr)?;
    Ok(match m.u8(def+3)? {
        2 => Some(states::integer_value(f32::from_bits(v),fcw)),
        1 => Some(v),
        0 => {
            let alpha = states::alpha_value(f32::from_bits(v),m.f32(engine::colour_pass::constant::PACK_SCALE)?,fcw);
            Some((m.u32(shadow)? & rgb_mask) | alpha)
        }
        _ => None,
    })
}

pub(super) async fn constants_states(cx:&Cx,cache:u32,leaf:Leaf,fcw:u16,mxcsr:u32) -> Result<(),EntryError> {
    for (index,kind) in [(3,Target::Pixel),(4,Target::Vertex),(1,Target::Render),(2,Target::Stage)] {
        for at in leaf.bindings(index) {
            let (property,target,operand) = cx.read(|m| -> Result<_,MemFault> {
                let refs = PropertyRefs::read(m,leaf,at)?;
                Ok((refs.reference(m,refs.range.start)?.0,m.u16(at)?,m.u8(at+2)?))
            })?;
            let mode = u32::from((target >> 5) & 31);
            let reference = property+12;
            let raw = if matches!(kind,Target::Vertex) && mode >= 5 {
                // Texture transform bindings read the resolved slot; they do
                // not evaluate the binding's property or its callback.
                0
            } else {
                value(cx,property,fcw,mxcsr).await?
            };
            match kind {
                Target::Pixel => {
                    let lane = operand & 7;
                    let packed = cx.read(|m| evaluate::pixel(m,reference,raw,lane,(operand >> 3) & 7,fcw,mxcsr)).map_err(error)?;
                    opaque_material_registers::write_colour(cx,cache,target,lane,packed)?;
                }
                Target::Vertex => {
                    let slot = u32::from(target & 15);
                    let bank = u32::from((target >> 4) & 1);
                    let dest = cache+VECTOR_BANK+bank*0x100+slot*16;
                    cx.write(|m| -> Result<(),Error> {
                        match mode {
                            0..=3 => {
                                let v = evaluate::ranged(m,reference,raw,mxcsr)?;
                                m.set_u32(dest+mode*4,v)?;
                            }
                            4 => {
                                let rgb = evaluate::rgb(m,reference,raw,fcw,mxcsr)?;
                                for (i,v) in rgb.into_iter().enumerate() { m.set_u32(dest+i as u32*4,v)?; }
                            }
                            _ => {
                                let tex = cache+TEXTURE_SLOTS + u32::from((target>>10)&7)*64;
                                evaluate::transform(m,tex,dest,mode,fcw,mxcsr)?;
                            }
                        }
                        let dirty = cache+VECTOR_DIRTY+bank*4;
                        Ok(m.set_u32(dirty,m.u32(dirty)? | (1<<slot))?)
                    }).map_err(error)?;
                }
                Target::Render | Target::Stage => {
                    let stage = u32::from(target as u8);
                    let selector = u32::from(target >> 8);
                    let (def,scalar) = match kind {
                        Target::Render => (states::DEFS+stage*4, selector==1),
                        _ => (STAGE_DEFS+selector*4, operand==1),
                    };
                    let (register,v) = cx.read(|m| -> Result<_,Error> {
                        let register = u32::from(m.u16(def)?);
                        let shadow = match kind {
                            Target::Render => states::SHADOW+register*4,
                            _ => STAGE_SHADOW+(stage*32+register)*4,
                        };
                        Ok((register,state_value(m,reference,raw,scalar,def,shadow,fcw,mxcsr)?))
                    }).map_err(error)?;
                    if let Some(v) = v {
                        match kind {
                            Target::Render => opaque_material_states::render_state(cx,register,v).await?,
                            _ => opaque_material_states::stage_state(cx,stage,register,v).await?,
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hooks::{EntryFuture, Ret};
    use guest_layout::entry::{Regs, testing::run};
    use engine::function_eval::constant::ONE;
    use guest_layout::testing::FakeGuest;

    const CACHE: u32 = 0x10000;
    const INSTANCE_DATA: u32 = 0x13000;
    const LEAF: u32 = 0x14000;
    const BINDINGS: u32 = 0x15000;
    const REFS: u32 = 0x16000;
    const RANGES: u32 = 0x17000;
    const PROPERTIES: u32 = 0x18000;
    const DATA: u32 = 0x19000;

    fn setup() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(CACHE,0x2000)
            .put_u32(INSTANCE_DATA+0x40,PROPERTIES)
            .put_u32(INSTANCE_DATA+0x48,REFS)
            .put_u32(INSTANCE_DATA+0x50,RANGES)
            .put_u32(INSTANCE_DATA+0x58,BINDINGS)
            .put_u32(INPUT_SOURCE,0)
            .put_f32(ONE,1.0)
            .put(engine::colour_pass::global::TIME,&2.5f64.to_le_bytes())
            .put_f32(PROPERTIES+8,2.0)
            .put_u32(PROPERTIES+12,16)
            .put_u32(PROPERTIES+16,DATA)
            .put_u32(DATA,0)
            .put_f32(DATA+4,1.0)
            .put_f32(DATA+8,5.0);
        m
    }

    #[test]
    fn prepare_selects_the_shader_instance_tree_by_lod_layer_pass_and_implementation() {
        let mut m = setup();
        let (lods, layers, passes, impls) = (0x40000, 0x41000, 0x42000, 0x43000);
        m.put_u32(INSTANCE_DATA+0x1c,2)
            .put_u32(INSTANCE_DATA+0x20,lods)
            .put_u32(INSTANCE_DATA+0x28,layers)
            .put_u32(INSTANCE_DATA+0x30,passes)
            .put_u32(INSTANCE_DATA+0x38,impls)
            .put_u32(lods+6,1<<3)
            .put_u16(lods+10,(4<<9) | 13)
            .put_u16(layers+16*2,(2<<9) | 8)
            .put_u16(passes+9*2,(3<<9) | 4)
            .put_u16(impls+6*10,(2<<9) | 3);
        let leaf = prepare(&m,INSTANCE_DATA,1,3,1,2).unwrap().unwrap();
        assert_eq!(leaf.bindings(0).collect::<Vec<_>>(),[BINDINGS+12,BINDINGS+16]);
        assert!(prepare(&m,INSTANCE_DATA,1,3,1,3).unwrap().is_none());
        assert!(prepare(&m,INSTANCE_DATA,1,3,2,1).unwrap().is_none());
        assert!(prepare(&m,INSTANCE_DATA,1,1,0,0).unwrap().is_none());
        assert!(prepare(&m,INSTANCE_DATA,0,3,1,2).unwrap().is_none());
    }

    #[test]
    fn time_curve_updates_two_texture_components_and_one_dirty_vertex_lane() {
        let mut m = setup();
        m.put_u16(LEAF,1 << 9)
            .put_u16(LEAF+8,(1 << 9) | 1)
            .put_u32(BINDINGS,2)
            .put_u32(BINDINGS+4,0x10 | 3 | (2 << 5))
            .put_u16(RANGES,2 << 9)
            .put_u16(REFS,0)
            .put_u16(REFS+2,0)
            .put_u16(REFS+4,0)
            .put_u16(REFS+6,1)
            .put_u32(CACHE+VECTOR_DIRTY+4,1<<11);
        let dest = CACHE+VECTOR_BANK+0x100+3*16;
        m.put_u32(dest+4,0x12345678);
        let run = run(&|cx: Cx| -> EntryFuture {
            Box::pin(async move {
                let leaf = cx.read(|m| Leaf::read(m, INSTANCE_DATA, LEAF))?.unwrap();
                textures(&cx,CACHE,leaf,0x37f,0x1f80).await?;
                constants_states(&cx,CACHE,leaf,0x37f,0x1f80).await?;
                Ok(Ret::new(0,0))
            })
        },m,Regs::default(),|_,_| panic!("ordinary time animation called the guest"));
        assert_eq!(run.result,Ok(Ret::new(0,0)));
        let slot = CACHE+TEXTURE_SLOTS+2*64+0x18;
        // Clock / period = 1.25, wrapped to 0.25, remapped from 1 to 5.
        assert_eq!(run.guest.f32(slot),Ok(2.0));
        assert_eq!(run.guest.f32(slot+4),Ok(2.0));
        assert_eq!(run.guest.u32(dest+4),Ok(0x12345678));
        assert_eq!(run.guest.f32(dest+8),Ok(2.0));
        assert_eq!(run.guest.u32(CACHE+VECTOR_DIRTY+4),Ok((1<<11)|(1<<3)));
    }

    #[test]
    fn active_input_source_calls_the_parameter_callback_for_a_time_property() {
        let mut m = setup();
        m.put_u32(INPUT_SOURCE,0x20000)
            .put_u32(0x20004,0x20100)
            .put_u16(LEAF,1<<9)
            .put_u16(RANGES,1<<9)
            .put_u32(REFS,0)
            .put_u32(BINDINGS,0);
        let run = run(&|cx: Cx| -> EntryFuture {
            Box::pin(async move {
                let leaf = cx.read(|m| Leaf::read(m, INSTANCE_DATA, LEAF))?.unwrap();
                textures(&cx,CACHE,leaf,0x37f,0x1f80).await?;
                Ok(Ret::new(0,0))
            })
        },m,Regs::default(),|call,_| {
            assert_eq!(call.target,PROPERTY_EVALUATOR);
            assert_eq!(call.eax,Some(PROPERTIES+12));
            assert_eq!(call.args,[0,2f32.to_bits()]);
            Regs { xmm0: u128::from(0.5f32.to_bits()), ..Regs::default() }
        });
        assert_eq!(run.calls.len(),1);
        assert_eq!(run.result,Ok(Ret::new(0,0)));
        assert_eq!(run.guest.f32(CACHE+TEXTURE_SLOTS+0x18),Ok(3.0));
    }
}
