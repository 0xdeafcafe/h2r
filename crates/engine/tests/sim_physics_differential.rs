//! Execute the user's XBE to the next call/return, then compare the Rust
//! segment with it. A child sees all registers, flags and the *original*
//! return address. Seeded child answers exercise callback clobbers and
//! changes to the live set before the next bitmap read.
#![cfg(feature = "differential")]

mod common;

use engine::points;
use engine::sim_physics::{PORTS, Port};
use ir::interp::{self, Cpu, FlatMemory, Flow};
use runtime::{CpuAbi, Guest, TransferAbi, abi, guest::Permissions};
use std::collections::BTreeMap;

const RAM: usize = 8 << 20;
const STACK: u32 = 0x007f_0000;
const EXIT: u32 = 0xffff_fffc;
const PHASE: u32 = 0x0060_0000;
const LIST: u32 = 0x0060_1000;
const HEADER: u32 = 0x0060_4000;
const DATA: u32 = 0x0060_5000;
const BITS: u32 = 0x0060_6000;
const RECORDS: u32 = 0x0061_0000;

fn word(m: &mut FlatMemory, a: u32, v: u32) {
    assert!(m.load(a, &v.to_le_bytes()));
}

fn fixture(m: &mut FlatMemory, case: u32) {
    word(m, 0x004d_e2f4, PHASE);
    word(m, 0x0051_07f0, LIST);
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, HEADER + 0x48, BITS);
    word(m, HEADER + 0x24, 12);
    word(m, LIST + 0x1d80, if case == 0 { 0 } else { 3 });
    for (i, slot) in [4, 2, 5].into_iter().enumerate() {
        word(m, LIST + 0x1d00 + i as u32 * 4, 0x8200_0000 | slot);
    }
    let high = [0, 31, 32, 33, 97, u32::MAX][case as usize % 6];
    word(m, HEADER + 0x38, high);
    for slot in 0..100 {
        word(m, DATA + slot * 12, ((slot << 4 | 7) << 16) | (0x8200 + slot));
        word(m, DATA + slot * 12 + 8, RECORDS + slot * 256);
        m.bytes[(RECORDS + slot * 256 + 0xc0) as usize] = (u8::from(slot % 3 == 1)) << 3;
    }
    word(m, BITS, if case == 2 { 0 } else { 1 << 31 | 4 });
    word(m, BITS + 4, 1);
    word(m, BITS + 8, 0);
    word(m, BITS + 12, u32::MAX);
    word(m, STACK, EXIT);
    word(m, STACK + 4, 0xdeaf_0002);
}

fn object_fixture(m: &mut FlatMemory, case: u32) {
    let component_header = DATA + 0x9a0;
    let components = RECORDS + 0x10000;
    let object = RECORDS + 0x300;
    word(m, points::GAME_GLOBALS.base, PHASE);
    m.bytes[(PHASE + 12) as usize] = 4;
    word(m, STACK + 4, 0x9100_0002);
    word(m, DATA + 2*12 + 8, object);
    word(m, DATA + 3*12 + 8, object + 0x400);
    m.bytes[(DATA + 2*12 + 2) as usize] = [0, 34, 2, 6, 66, 2][case as usize];
    m.bytes[(DATA + 2*12 + 3) as usize] = if case >= 4 { 7 } else { 1 };
    word(m, object + 0x10, if case == 3 { 0x8400_0003 } else { u32::MAX });
    word(m, object + 0x400 + 12, u32::MAX);
    word(m, object + 180, if case >= 4 { 2 } else { u32::MAX });
    m.bytes[(object + 216) as usize] = (case & 1) as u8;
    m.bytes[(object + 192) as usize] = [0, 0, 4, 2, 64, 68][case as usize];
    m.bytes[(object + 179) as usize] = if case == 5 { 0x80 } else { 0 };
    assert!(m.load(object + 276, &104u16.to_le_bytes()));
    let link: u16 = if case == 3 { 0x170 } else { u16::MAX };
    assert!(m.load(object + 298, &link.to_le_bytes()));
    word(m, object + 0x170 + 104, 0x9000);
    word(m, 0x0051_e9b8, component_header);
    word(m, component_header + 0x44, components);
    word(m, components + 2*0xa0 + 4, if case == 5 { 1 << 15 } else { 0 });
}

const SUPPORT_CHILD: u32 = 0x006e_0000;

fn support_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let (shape,table,ids,matrix) = (DATA,DATA+64,DATA+256,DATA+512);
    c.gpr[1] = RECORDS;
    for (i,p) in [shape,shape,matrix,ids,0].into_iter().enumerate() {
        word(m,STACK+4+4*i as u32,p);
    }
    word(m,shape,table);
    word(m,table+52,SUPPORT_CHILD);
    m.bytes[(ids+8) as usize] = 1;
    m.bytes[(ids+9) as usize] = (case%4) as u8;
    for i in 0..16 {
        word(m,matrix+4*i,((i as f32 - 5.0)/3.0).to_bits());
        word(m,RECORDS+224+4*i,((7.0-i as f32)/13.0).to_bits());
    }
    c.mxcsr |= (case%4)<<13;
}

fn solver_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let (param,schema,rows,out,acc,imp) = (DATA,DATA+128,DATA+512,DATA+1024,RECORDS,RECORDS+1024);
    for (i,p) in [param,schema,acc,imp].into_iter().enumerate() {
        word(m,STACK+4+4*i as u32,p);
    }
    m.bytes[0x480c60] = u8::from(case != 0);
    word(m,param+28,u32::from(case>=4));
    for (off,v) in [(4,0.75f32),(8,0.125),(16,1.25)] { word(m,param+off,v.to_bits()); }
    for (off,v) in [(4,rows),(8,out),(12,acc),(16,acc+128),(20,12)] { word(m,schema+off,v); }
    // Select schema IDs by the destinations in the caller's own image.
    let lookup = |m: &FlatMemory, form: u32| -> u8 {
        (0..=16u32).find(|i| {
            let class = m.bytes[(0x31e724+i) as usize];
            m.read_u32(0x31e704+4*u32::from(class)) == Some(form)
        }).expect("schema class") as u8
    };
    let done = lookup(m,0x31e696);
    let mut p = schema+24;
    if (1..4).contains(&case) {
        let forms: &[u32] = match case {
            1 => &[0x31e567,0x31e567,0x31e5aa],
            2 => &[0x31e52a,0x31e5ed],
            _ => &[0x31e631],
        };
        for &form in forms {
            let size = match form { 0x31e52a => 8, 0x31e5ed => 20, 0x31e631 => 24, _ => 4 };
            let id = lookup(m,form);
            m.bytes[p as usize] = id;
            m.bytes[(p+1) as usize] = size as u8;
            if size >= 20 { word(m,p+4,out+128); word(m,p+16,12); }
            p += size;
        }
    } else if case >= 4 {
        let id = (0..=16u32).find(|i| m.read_u32(0x31e6a4+4*i) == Some(0x31cf97)).unwrap() as u8;
        for _ in 0..2 { m.bytes[p as usize] = id; p += 4; }
    }
    m.bytes[p as usize] = done;
    for i in 0..64 {
        word(m,rows+4*i,((i as f32+0.5)/9.0).to_bits());
        word(m,acc+4*i,((i as f32-25.0)/17.0).to_bits());
        word(m,imp+4*i,((i as f32+1.0)/7.0).to_bits());
    }
    if case==5 { word(m,rows+12,(-200f32).to_bits()); }
    c.x87.control = [0x007f,0x027f,0x037f][case as usize%3];
    c.mxcsr |= (case%4)<<13;
}

fn light_fixture(m:&mut FlatMemory, case:u32) {
    let (lights,light,def,tags,object,clock)=(DATA+0x9000,RECORDS+0x7000,DATA+0xa000,DATA+0xb000,RECORDS+0xa000,DATA+0xc000);
    word(m,0x4e030c,lights);
    word(m,lights+68,light);
    word(m,STACK+4,0);
    word(m,light+4,1);
    word(m,points::TAG_ENTRIES.base,tags);
    word(m,tags+16+8,def);
    word(m,DATA+12+8,object);
    word(m,light+76,if case>=3 {1} else {u32::MAX});
    word(m,light+88,u32::MAX);
    word(m,light+84,if case==2 {1} else {65535});
    m.bytes[(light+2) as usize]=[0,1,1,16,3,10][case as usize];
    word(m,light+204,0.5f32.to_bits());
    for off in (64..120).step_by(4) {
        word(m,def+off,((off as f32-80.0)/17.0).to_bits());
    }
    for (off,v) in [(8,0.5f32),(12,0.25),(176,1.5)] {word(m,def+off,v.to_bits());}
    word(m,0x510c54,clock);
    word(m,clock,30<<16);
    word(m,clock+8,19);
    word(m,object+688,0.4f32.to_bits());
    m.bytes[(object+170) as usize]=0;
}

fn first_person_fixture(m:&mut FlatMemory,case:u32) {
    let (user,palette,identity,weapon,object,tags)=(RECORDS,RECORDS+0x10000,DATA+0x5000,DATA+0x6000,DATA+0x7000,DATA+0x8000);
    let (variant,model,body,globals,types,clock)=(DATA+0x9000,DATA+0xa000,DATA+0xb000,DATA+0xc000,DATA+0xd000,DATA+0xe000);
    word(m,STACK+4,0);word(m,STACK+8,0);
    word(m,0x4e9bc8,user);word(m,0x4e9bc4,palette);word(m,0x4687d8,identity);
    word(m,user+4,if case==0 {u32::MAX} else {1});
    let slot=user+12;
    word(m,slot+4,if case==0 {u32::MAX} else {1});
    word(m,user+8,0);word(m,DATA+12+8,object);word(m,object,2);
    word(m,points::TAG_ENTRIES.base,tags);word(m,tags+2*16+8,weapon);
    word(m,weapon+684,variant);word(m,variant+4,3);word(m,tags+3*16+8,model);
    word(m,0x4e034c,globals);word(m,globals+316,types);word(m,types+4,4);
    word(m,tags+4*16+8,body);
    for (i,def) in [model,body].into_iter().enumerate() {
        let count=case.saturating_sub(1)%4;
        let nodes=DATA+0xf000+i as u32*512;
        word(m,def+72,count);word(m,def+76,nodes);
        for j in 0..3 {
            word(m,slot+252+256*i as u32+4*j,if j==1 {65535} else {j});
            for k in 0..24 {word(m,nodes+96*j+4*k,0xabc0+j*90+k+i as u32);}
        }
        word(m,slot+244+4*i as u32,if case==3 {0} else {count});
    }
    for j in 0..8 {word(m,identity+4*j,0x2000+j);}
    word(m,0x510c54,clock);word(m,clock+4,(1.0f32/30.0).to_bits());
    for (i,v) in [0.25f32,-0.0,1.0,f32::NAN].into_iter().enumerate() {
        word(m,weapon+736+4*i as u32,v.to_bits());
    }
}

fn observer_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let view = RECORDS;
    let axis = RECORDS + 0x400;
    c.gpr[6] = view;
    word(m, 0x0046_87b0, axis);
    word(m, 0x0054_e854, 1.25f32.to_bits());
    for (i, v) in [0.4f32, 0.6, -0.7].into_iter().enumerate() {
        word(m, axis + 4*i as u32, v.to_bits());
    }
    for (i,v) in [0.43f32, 0.8, -0.2, 0.5, -0.3, 0.75,
                   case as f32 / 3.0, 1.75, -0.5, 0.75].into_iter().enumerate() {
        word(m, view + 32 + 4*i as u32, v.to_bits());
    }
    c.x87.control = [0x007f, 0x027f, 0x037f][case as usize % 3];
    c.mxcsr |= (case % 4) << 13;
}

fn color_fixture(m: &mut FlatMemory, case: u32) {
    let object = RECORDS + 0x400;
    let tags = RECORDS + 0x6000;
    let definition = tags + 256;
    let channels = tags + 768;
    let functions = tags + 1024;
    word(m, object, 1);
    word(m, STACK + 4, 2);
    word(m, DATA + 2*12 + 8, object);
    word(m, points::TAG_ENTRIES.base, tags);
    word(m, tags + 16 + 8, definition);
    word(m, definition + 176, channels);
    m.bytes[(definition+28) as usize] = u8::from(case > 0);
    assert!(m.load(object + 292, &(if case == 1 { 0u16 } else { 48 }).to_le_bytes()));
    assert!(m.load(object + 294, &512u16.to_le_bytes()));
    for i in 0..6 {
        word(m, object + 512 + 4*i, [0.25f32,-0.0,1.5,-0.5,f32::NAN,0.7][i as usize].to_bits());
    }
    for i in 0..2 {
        word(m, channels + 16*i + 8, case.saturating_sub(2));
        word(m, channels + 16*i + 12, functions);
    }
    for i in 0..3 {
        word(m, functions + 40*i + 36, i+1);
        word(m, functions + 40*i + 32, i);
    }
}

fn visit_fixture(m: &mut FlatMemory, case: u32) {
    let center = RECORDS + 0x8000;
    let location = center + 64;
    let out = location + 64;
    for (i, arg) in [
        [0, 1, 2, 3, 3, 3][case as usize], 0, location, center,
        [0.0f32, -0.0, f32::NAN, 1.5, 3.0, 1.0][case as usize].to_bits(), out,
        [8, 8, 8, 1, 4, 0][case as usize],
    ].into_iter().enumerate() { word(m, STACK + 4 + 4 * i as u32, arg); }
    assert!(m.load(location + 4, &(if case == 0 { u16::MAX } else { 3 }).to_le_bytes()));
    for &table in &[0x004d_e2e4, 0x004d_e2d8] { word(m, table, HEADER); }
    word(m, HEADER + 0x24, 64);
    word(m, HEADER + 0x44, DATA);
    for i in 0..3 {
        let p = DATA + i*64;
        word(m, p + 4, 0x8100_0000 | i);
        word(m, p + 8, if i == 2 { u32::MAX } else { i+1 });
        m.bytes[(p+12) as usize] = (i+1) as u8;
        word(m, p + 16, (i as f32 * 0.25).to_bits());
        word(m, p + 28, 2.0f32.to_bits());
    }
    word(m, 0x004d_e2fc, 7);
}

fn portal_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let (bsp, portals, planes, center, geometry) = (DATA, DATA + 128, DATA + 384, DATA + 512, DATA + 640);
    word(m, bsp + 0x60, portals);
    word(m, bsp + 0x18, geometry);
    word(m, geometry + 12, planes);
    word(m, 0x004e_033c, geometry);
    let portal = portals + 36;
    word(m, portal + 4, 0);
    word(m, portal + 28, if case == 5 { 0 } else { 4 });
    word(m, portal + 32, DATA + 768);
    for i in 0..12 {
        word(m, DATA + 768 + 4*i, ((i as f32 - 4.25) * 1.7).to_bits());
    }
    let normal = if case == 4 { f32::NAN } else { -1.0 };
    word(m, planes, normal.to_bits());
    word(m, planes + 4, 0.0f32.to_bits());
    word(m, planes + 8, 0.0f32.to_bits());
    word(m, planes + 12, 0.0f32.to_bits());
    word(m, center, 1.0f32.to_bits());
    word(m, center + 4, (-0.75f32).to_bits());
    word(m, center + 8, 0.25f32.to_bits());
    word(m, portal + 16, if case == 2 { 20.0f32 } else { 0.0 }.to_bits());
    word(m, portal + 20, 1.0f32.to_bits());
    let radius: f32 = match case { 0 => -1.0, 1 => 1.0, _ => 3.75 };
    word(m, STACK + 4, radius.to_bits());
    c.gpr[0] = 0xabc0_0001;
    c.gpr[1] = center;
    c.gpr[2] = bsp;
    c.x87.control = [0x037f, 0x007f, 0x027f][case as usize % 3];
    c.mxcsr |= (case % 4) << 13;
}

fn abi_cpu(c: &Cpu) -> CpuAbi {
    let [eax, ecx, edx, ebx, esp, ebp, esi, edi] = c.gpr;
    let x = &c.x87;
    CpuAbi {
        eax, ecx, edx, ebx, esp, ebp, esi, edi,
        eip: c.eip, fs_base: c.fs_base, mxcsr: c.mxcsr,
        eflags: c.eflags, eflags_defined: c.flags_valid,
        xmm: c.xmm.map(u128::to_le_bytes),
        x87: runtime::cpu::X87Abi {
            st: x.st, control: x.control, status: x.status, tag: x.tag, opcode: x.opcode,
            instruction_pointer: x.instruction_pointer, data_pointer: x.data_pointer,
            instruction_selector: x.instruction_selector, data_selector: x.data_selector,
        },
        ..Default::default()
    }
}

fn segment(insns: &BTreeMap<u32, ir::IrInsn>, cpu: &mut Cpu, mem: &mut FlatMemory, stop: Option<(u32, u32)>) -> TransferAbi {
    for _ in 0..500_000 {
        let pc = cpu.eip;
        let i = insns.get(&pc).unwrap_or_else(|| panic!("missing instruction at {pc:#x}"));
        let flow = interp::step(cpu, mem, i).unwrap_or_else(|e| panic!("{pc:#x}: {e:?}"));
        cpu.eip = flow.target();
        if Some((cpu.eip, cpu.gpr[6])) == stop { return TransferAbi::resume(cpu.eip); }
        match flow {
            Flow::Return { target } => return TransferAbi::ret(target),
            Flow::Call { target, continuation } => return TransferAbi::call(target, continuation),
            _ => {}
        }
    }
    panic!("did not reach a call or return");
}

fn compare(port: &Port, insns: &BTreeMap<u32, ir::IrInsn>, mut mem: FlatMemory, mut cpu: Cpu, case: u32) {
    let mut native = Guest::new();
    native.map_zeroed(0, RAM as u32, Permissions::RW).unwrap();
    native.write_bytes(0, &mem.bytes, 1).unwrap();
    let start = native.mappings()[0].physical_start as usize;
    let mut regs = abi_cpu(&cpu);
    for n in 0..1024 {
        let at = cpu.eip;
        let actual = (port.run)(&mut regs, &mut native, at).unwrap();
        let stop = (actual.route == abi::RESUME).then_some(actual.target_pc);
        if let Some(pc) = stop { assert!(port.generated_sites.contains(&pc)); }
        let expected = segment(insns, &mut cpu, &mut mem, stop.map(|pc| (pc, regs.esi)));
        let why = || format!("{} case {case} boundary {n} from {at:#x}", port.switch);
        assert_eq!(actual, expected, "{}", why());
        regs.eip = actual.target_pc;
        assert_eq!(regs, abi_cpu(&cpu), "{}", why());
        let ram = &native.physical_ram()[start..start + RAM];
        if mem.bytes != ram {
            let first = mem.bytes.iter().zip(ram).position(|(a,b)| a != b).unwrap();
            panic!("{} first different byte at {first:#x}", why());
        }
        if stop.is_some() || (actual.kind == abi::CALL && port.generated_sites.contains(&actual.resume_pc)) { return; }
        if actual.kind == abi::RETURN {
            assert_eq!(actual.target_pc, EXIT);
            return;
        }
        let target = expected.target_pc;
        let cleanup = match target {
            0xbc470 | 0xbc820 | 0x109580 | 0xc49b0 | 0xc58f0 | 0xc60c0 => 4,
            0x23a160 | 0x108c60 | 0xd5de0 | 0xbe8e0 | 0xbc5e0 | 0xbd090 | 0xbdef0 => 4,
            0x1d3920 | 0xbab40 => 12,
            0x131c20 | 0x31c7d0 => 0,
            0x31c520 | 0x31c530 => 4,
            SUPPORT_CHILD => 12,
            0x16898b | 0x1c62f0 | 0x1cbd30 | 0x166561 => 0,
            0x167e86 => 8, 0x11eed0 => 20,
            0xbad50 | 0x17ca10 | 0xbaf80 | 0xb9ce0 | 0xb9d20 | 0xc3220 => 0,
            0xc2d00 => 4, 0xc0b00 => 16, 0x131d60 => 8,
            0xbef30 | 0x14a5b0 => 20,
            0xb8940 | 0xb8a10 => 8,
            0xbba20 | 0xa9570 | 0x1cd4e0 | 0x1ced00 | 0x108cd0 | 0xbe1d0 => 0,
            0x120850 | 0x109660 | 0xfd910 | 0xc5eb0 | 0xc6740 | 0x114c60 | 0xc6990 | 0xc6810 | 0xc50a0 => 0,
            _ => panic!("unexpected callee {target:#x}"),
        };
        let sp = cpu.gpr[4];
        let next = mem.read_u32(sp).unwrap();
        assert_eq!(next, actual.resume_pc);
        // A callback unlinks a future slot, grows or shrinks the bitmap, or
        // detaches the listed object. Both sides must re-read those fields.
        if matches!(target, 0xbc470 | 0xbc820) {
            let slot = mem.read_u32(sp + 4).unwrap() & 0xffff;
            if n == 0 && case & 1 != 0 {
                for (a, v) in [(BITS + 4, 0b110), (HEADER + 0x38, 35)] {
                    word(&mut mem, a, v);
                    native.set_u32(a, v).unwrap();
                }
                mem.bytes[(DATA + slot * 12 + 2) as usize] = 8;
                native.set_u8(DATA + slot * 12 + 2, 8).unwrap();
            }
        }
        if matches!(target, 0xb8940 | 0xb8a10) {
            for (a,v) in [(cpu.gpr[3], 1), (mem.read_u32(sp+8).unwrap(), DATA+12)] {
                word(&mut mem, a, v);
                native.set_u32(a,v).unwrap();
            }
        }
        if target == 0x14a5b0 {
            let out = mem.read_u32(sp+20).unwrap();
            word(&mut mem, out, (5 << 16) | 2);
            native.set_u32(out, (5 << 16) | 2).unwrap();
        }
        if target == 0xbab40 {
            let out = mem.read_u32(sp + 12).unwrap();
            let value = (-0.75f32 + n as f32 * 0.25).to_bits();
            word(&mut mem, out, value);
            native.set_u32(out, value).unwrap();
        }
        let answer = target.wrapping_mul(0x9127).rotate_left(n % 32) ^ case;
        cpu.gpr[0] = match target {
            0x31c520 | 0x31c530 => cpu.gpr[1] + mem.read_u32(sp+4).unwrap()
                * if target == 0x31c520 {32} else {48},
            0x16898b => 0, 0x1cbd30 => case%4,
            0xbaf80 => 1,
            0x120850 => case % 3,
            0xbab40 => n % 2,
            0x14a5b0 => 2,
            0xb8940 | 0xb8a10 => 0x8100_0000,
            _ => answer,
        };
        cpu.gpr[1] = answer.rotate_left(9);
        // 16898b is a leaf called with the live tag table in EDX.
        if target != 0x16898b {cpu.gpr[2] = !answer;}
        cpu.gpr[4] = sp + 4 + cleanup;
        cpu.eip = next;
        regs = abi_cpu(&cpu);
    }
    panic!("unbounded callback loop");
}

#[test]
fn native_simulation_matches_xbe() {
    let Some(path) = common::xbe_path() else {
        assert!(std::env::var_os("H2_XBE").is_none(), "H2_XBE does not name a readable file");
        eprintln!("SKIPPED native_simulation_matches_xbe: set H2_XBE");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(common::sha256(&bytes), common::IMAGE_SHA256);
    let xbe = xbe::Xbe::parse(&bytes).unwrap();
    let analysis = analyze::analyze(&xbe).unwrap();
    let lifter = lift::Lifter::new(&analysis, &xbe).unwrap();
    let mut image = lift::harness::load_image(&xbe).unwrap();
    image.bytes.truncate(RAM);
    for port in PORTS {
        let function = lifter.lift(port.point.base).expect(port.point.name);
        let insns = function.blocks.into_values().flat_map(|b| b.insns)
            .map(|i| (i.pc, i)).collect();
        for case in 0..6 {
            let mut m = image.clone();
            fixture(&mut m, case);
            let mut cpu = Cpu {
                gpr: [0x12345678, 0x98765432, 0xfe3420, 0x7b1c, STACK, 0xaabbcc, 0x5151, 0xe37],
                eip: port.point.base,
                eflags: 0xa93, ..Default::default()
            };
            cpu.xmm[0] = 0x1234 << 100;
            if [points::OBJECT_PRE_UPDATE.base, points::OBJECT_POST_UPDATE.base].contains(&port.point.base) {
                object_fixture(&mut m, case);
            }
            if [points::FIRST_PERSON_POSE_UPDATE.base,points::FIRST_PERSON_FILTER_UPDATE.base].contains(&port.point.base) {
                first_person_fixture(&mut m,case);
            }
            if port.point.base == points::LIGHT_RECORD_UPDATE.base { light_fixture(&mut m,case); }
            if port.point.base == points::HAVOK_SOLVER.base { solver_fixture(&mut m,&mut cpu,case); }
            if port.point.base == points::HAVOK_SUPPORT_VERTICES.base { support_fixture(&mut m,&mut cpu,case); }
            if port.point.base == points::OBSERVER_RESULT.base { observer_fixture(&mut m, &mut cpu, case); }
            if port.point.base == points::OBJECT_CHANGE_COLORS.base { color_fixture(&mut m, case); }
            if port.point.base == points::OBJECT_SPATIAL_VISIT.base { visit_fixture(&mut m, case); }
            if port.point.base == points::OBJECT_SPATIAL_PORTAL.base { portal_fixture(&mut m, &mut cpu, case); }
            compare(port, &insns, m, cpu, case);
        }
    }
}
