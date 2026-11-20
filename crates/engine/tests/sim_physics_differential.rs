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

const SOLVER_CHILD: u32 = 0x006e_0020;
const SUPPORT_CHILD: u32 = 0x006e_0000;
const SUPPORT_QUERY: u32 = 0x006e_0010;

fn support_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let (shape,table,ids,matrix) = (DATA,DATA+64,DATA+256,DATA+512);
    c.gpr[1] = RECORDS;
    for (i,p) in [shape,shape,matrix,ids,DATA+2048].into_iter().enumerate() {
        word(m,STACK+4+4*i as u32,p);
    }
    // Two instances of a synthetic tetrahedron use the same virtual ABI.
    let other = shape+96;
    word(m,STACK+8,other);
    word(m,other,table);
    word(m,shape,table);
    word(m,table+52,SUPPORT_CHILD);
    word(m,table+48,SUPPORT_QUERY);
    let (a,b)=[(1,1),(1,2),(2,2),(3,1),(1,3),(2,3)][case as usize];
    for i in 0..4u16 {
        assert!(m.load(ids+2*u32::from(i), &(i+case as u16).to_le_bytes()));
    }
    m.bytes[(ids+8) as usize]=a;m.bytes[(ids+9) as usize]=b;
    m.bytes[(ids+10) as usize]=a+u8::from(case==0);
    m.bytes[(ids+11) as usize]=b+u8::from(case==1);
    m.bytes[(RECORDS+12) as usize]=(case%2) as u8;
    for i in 0..12 {word(m,RECORDS+32+4*i,((i as f32+1.0)/7.0).to_bits());}
    word(m,RECORDS+288,0.75f32.to_bits());
    for i in 0..16 {
        word(m,matrix+4*i,((i as f32 - 5.0)/3.0).to_bits());
        word(m,RECORDS+224+4*i,((7.0-i as f32)/13.0).to_bits());
    }
    c.mxcsr |= (case%4)<<13;
}

// Return packed vertices for the synthetic support callbacks. The fourth
// lane carries the vertex ID and participates in the guest packed math.
fn support_vertices(mem: &mut FlatMemory, native: &mut Guest, cpu: &Cpu, target: u32) {
    let points = [[0.25f32,0.75,-0.125],[-0.5,0.375,0.5],[0.875,-0.25,0.125],[-0.25,-0.5,-0.75]];
    let sp = cpu.gpr[4];
    let other = cpu.gpr[1] == DATA+96;
    let write = |mem: &mut FlatMemory, native: &mut Guest, dest: u32, id: usize| {
        let point = points[id];
        for (i,value) in point.into_iter().enumerate() {
            let bits = if other {-value} else {value}.to_bits();
            word(mem,dest+4*i as u32,bits);
            native.set_u32(dest+4*i as u32,bits).unwrap();
        }
        word(mem,dest+12,id as u32);
        native.set_u32(dest+12,id as u32).unwrap();
    };
    let arg = mem.read_u32(sp+4).unwrap();
    if target == SUPPORT_CHILD {
        let count = mem.read_u32(sp+8).unwrap();
        let dest = mem.read_u32(sp+12).unwrap();
        assert!(count<=4);
        for i in 0..count {
            let index = (mem.read_u32(arg+2*i).unwrap() & 3) as usize;
            write(mem,native,dest+16*i,index);
        }
    } else {
        let direction = [0,4,8].map(|off| f32::from_bits(mem.read_u32(arg+off).unwrap()));
        let dot = |v: &[f32;3]| {
            let d = direction[0]*v[0]+direction[1]*v[1]+direction[2]*v[2];
            if other {-d} else {d}
        };
        let mut best=0;
        for i in 1..points.len() {
            if dot(&points[i])>dot(&points[best]) {best=i;}
        }
        let dest=mem.read_u32(sp+8).unwrap();
        write(mem,native,dest,best);
    }
}

fn solver_fixture(m: &mut FlatMemory, c: &mut Cpu, case: u32) {
    let (param,schema,rows,out,acc,imp) = (DATA,DATA+128,DATA+512,DATA+1024,RECORDS,RECORDS+1024);
    m.bytes[DATA as usize..(DATA+2048) as usize].fill(0);
    for (i,p) in [param,schema,acc,imp].into_iter().enumerate() {
        word(m,STACK+4+4*i as u32,p);
    }
    m.bytes[0x480c60] = u8::from(case != 0);
    word(m,param+28,if case>=6 {2} else {u32::from(case>=4)});
    for (off,v) in [(4,0.75f32),(8,0.125),(12,0.8),(16,1.25)] { word(m,param+off,v.to_bits()); }
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
        // Build the synthetic stream from handler identities, independent of
        // the image's numeric dispatch tables. Fields not consumed stay zero.
        let instructions: &[u32] = match case {
            4|5=>&[0x31cf97,0x31cf97],
            6=>&[0x31da55,0x31db21],
            7=>&[0x31d982,0x31df3e],
            8=>&[0x31dc3f,0x31d796],
            9=>&[0x31d88e,0x31dd80],
            10..=13=>&[0x31c9c6,0x31c9c6],
            14..=16=>&[0x31d532],
            17..=19=>&[0x31d1fb],
            20=>&[0x31e068,0x31cf97,0x31e08d],
            21=>&[0x31e0a6],
            _=>&[0x31c9a0,0x31df3e,0x31df3e],
        };
        for &handler in instructions {
            let (size,correction)=match handler {
                0x31cf97|0x31db21=>(4,0x31e400),
                0x31da55=>(4,0x31e472),
                0x31d982|0x31d88e=>(16,0x31e472),
                0x31df3e=>(16,0x31e400),
                0x31dc3f=>(8,0x31e400),0x31d796=>(8,0x31e472),
                0x31dd80=>(12,0x31e400),0x31c9c6=>(8,0x31e161),
                0x31d532=>(20,0x31e161),0x31d1fb=>(24,0x31e2e9),
                0x31c9a0=>(24,0x31e0e1),0x31e068=>(12,0x31e4cf),
                0x31e08d=>(4,0x31e4cf),_ => (12,0x31e4cf),
            };
            let id=(0..=16u32).find(|i| m.read_u32(0x31e6a4+4*i)==Some(handler)).unwrap();
            let pass=(0..=6u32).find(|i| m.read_u32(0x31e6e8+4*i)==Some(correction)).unwrap();
            word(m,p,id|(size<<8)|(pass<<16));
            if size>=8 {word(m,p+4,0.2f32.to_bits());}
            if size>=12 {word(m,p+8,(-0.3f32).to_bits());}
            if size>=16 {word(m,p+12,if case%2==0 {0.02f32} else {9.0}.to_bits());}
            if size>=20 {word(m,p+4,out+128);word(m,p+16,12);}
            if size==24 {word(m,p+20,1.125f32.to_bits());}
            if handler==0x31e0a6 {word(m,p+4,SOLVER_CHILD);}
            if handler==0x31c9a0 {
                for (off,v) in [(4,rows),(8,out),(12,acc),(16,acc+128),(20,12)] {word(m,p+off,v);}
            }
            p+=size;
        }
    }
    let end=(0..=6u32).find(|i| m.read_u32(0x31e6e8+4*i)==Some(0x31e4da)).unwrap();
    word(m,p,u32::from(done)|(end<<16));
    for i in 0..64 {
        word(m,rows+4*i,((i as f32+0.5)/9.0).to_bits());
        word(m,acc+4*i,((i as f32-25.0)/17.0).to_bits());
        word(m,imp+4*i,((i as f32+1.0)/7.0).to_bits());
    }
    m.bytes[acc as usize]=2;
    m.bytes[(acc+128) as usize]=2;
    m.bytes[(acc+256) as usize]=3;
    if case==5 { word(m,rows+12,(-200f32).to_bits()); }
    if (10..=19).contains(&case) {
        for i in 0..4 {
            word(m,rows+48*i+12,if (case+i)%3==0 {-2.0f32} else {4.0}.to_bits());
        }
    }
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
    m.bytes[(user+8237) as usize]=u8::from(case>=3);
    word(m,0x4ed284,DATA+0x13000);word(m,0x4687c4,DATA+0x13100);
    word(m,user+4124+4,u32::MAX);
    word(m,user+8340,if case%2==0 {0} else {u32::MAX});
    for (j,v) in [0.3f32,-0.2,0.45,-0.7].into_iter().enumerate(){
        word(m,user+[8240,8244,8256,8260][j],v.to_bits());
    }
    word(m,slot+228,0.25f32.to_bits());
    word(m,DATA+0x12000+20,11);
    for off in [218,222,226,764,766] {
        let value:u16=if case<3 {u16::MAX} else {1};
        assert!(m.load(slot+off,&value.to_le_bytes()));
    }
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

const WELD_SHAPE: u32 = 0x6e0100;
const WELD_TYPE: u32 = 0x6e0110;
const WELD_ACCEPT: u32 = 0x6e0120;
const WELD_QUERY: u32 = 0x6e0130;
const WELD_REJECT: u32 = 0x6e0140;
const WELD_AGENT: u32 = RECORDS+0x4000;
const CONTACT_STORAGE:u32 = RECORDS+0x8000;

fn weld_fixture(m:&mut FlatMemory,c:&mut Cpu,case:u32) {
    c.fs_base=DATA+0x4000;
    c.gpr[1]=DATA;
    word(m,0x480154,0);word(m,0x480158,0);
    let (a,b,config,records,shape,vtable)=(DATA+128,DATA+256,DATA+512,DATA+768,DATA+1024,DATA+1280);
    for (i,v) in [a,b,config,DATA+3000].into_iter().enumerate() {word(m,STACK+4+4*i as u32,v);}
    word(m,a,shape);word(m,b,DATA+1536);word(m,DATA+1536+12,DATA+1792);word(m,DATA+1792+12,shape);
    word(m,b+8,DATA+2048);word(m,DATA+12,records);word(m,DATA+16,u32::from(case>=4));
    word(m,DATA+8,DATA+2304);word(m,records+8,WELD_AGENT);word(m,WELD_AGENT,vtable);
    word(m,WELD_AGENT+24,u32::from(case==3));word(m,shape,vtable);
    for (off,target) in [(52,WELD_SHAPE),(20,WELD_TYPE),(16,WELD_REJECT),(32,WELD_QUERY)] {word(m,vtable+off,target);}
    word(m,0x461f0c,WELD_ACCEPT);
    m.bytes[(DATA+2560) as usize]=u8::from(case!=4);
    for i in 0..5 {
        let p=CONTACT_STORAGE+128*i;
        for j in 0..32 {word(m,p+j*4,0x4000+i*64+j);}
        word(m,p+32,[2.0f32,1.0,1.0,-0.5,f32::NAN][i as usize].to_bits());
        word(m,p+112,WELD_AGENT);
        m.bytes[(p+125) as usize]=(i%2) as u8;
    }
}

fn aim_fixture(m:&mut FlatMemory,c:&mut Cpu,case:u32) {
    let (joint,from,to,rate,bounds)=(RECORDS,DATA+512,DATA+1024,DATA+1536,DATA+2048);
    let f=|m:&mut FlatMemory,p,v:f32|word(m,p,v.to_bits());
    for (i,v) in [0.9f32,-0.13,0.25,0.22,0.85,-0.09,0.10,0.17,0.8].into_iter().enumerate() {
        f(m,joint+4+4*i as u32,v);
    }
    for (p,vec) in [(from,[0.9f32,0.4,0.1]),(to,[0.7,-0.2,0.35]),
            (rate,if case%3==0 {[0.0;3]} else {[0.2,-0.1,0.15]})] {
        for (i,v) in vec.into_iter().enumerate(){f(m,p+4*i as u32,v);}
    }
    for (i,v) in [if case==4 {-4.0} else {-1.0},if case==4 {4.0} else {1.0},
                    if case==1 || case==5 {0.8} else {-1.0},1.0].into_iter().enumerate() {
        f(m,bounds+4*i as u32,v);
    }
    c.gpr[0]=to;c.gpr[6]=if case%2==0 {0} else {joint};c.gpr[7]=from;
    for (i,v) in [0.033f32.to_bits(),rate,bounds,0.8f32.to_bits(),0.9f32.to_bits()].into_iter().enumerate(){
        word(m,STACK+4+4*i as u32,v);
    }
    c.x87.control=[0x007f,0x027f,0x037f][case as usize%3];c.mxcsr |= (case%4)<<13;
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

/// One object with a short skeleton. Orientations are unit-ish quaternions
/// with offsets and scales, so the inlined pose and composition leaves do
/// real arithmetic. Case 3 has no node block and no nodes, case 4 no model,
/// case 5 is a biped the guest leaves alone.
fn node_fixture(m: &mut FlatMemory, _: &mut Cpu, case: u32) {
    let tags = RECORDS;
    let object = RECORDS + 0x1000;
    let parent = RECORDS + 0x1800;
    let definition = RECORDS + 0x2000;
    let model = RECORDS + 0x2100;
    let skeleton = RECORDS + 0x2200;
    let nodes = RECORDS + 0x2400;
    let block = 0x200;
    let nodes_count = [5u32, 7, 6, 0, 4, 3][case as usize];
    let matrices = 0x400u32;
    word(m, points::OBJECTS.base, HEADER);
    word(m, HEADER + 0x44, DATA);
    word(m, points::TAG_ENTRIES.base, tags);
    word(m, STACK + 4, 0x8200_0002);
    word(m, DATA + 2*12 + 8, object);
    word(m, DATA + 5*12 + 8, parent);
    for (i, data) in [(1u32, definition), (2, model), (3, skeleton)] {
        word(m, tags + i*16 + 8, data);
    }
    word(m, object, 1);
    word(m, object + 4, [0, 0x4000_0400, 0x4000_0000, 0x2000_0400, 0, 0][case as usize]);
    word(m, object + 0x14, if matches!(case, 1 | 4) { 5 } else { u32::MAX });
    m.bytes[(object + 0x18) as usize] = 1;
    for i in 0..9 {
        word(m, object + 0x64 + 4*i, ((i as f32 - 3.5) / 4.0).to_bits());
    }
    word(m, object + 0xa0, 0.875f32.to_bits());
    word(m, object + 0xb4, if matches!(case, 1 | 2) { 3 } else { u32::MAX });
    m.bytes[(object + 0xc0) as usize] = 0x40;
    word(m, object + 0xd4, if case == 1 { 9 } else { u32::MAX });
    m.bytes[(object + 0xd8) as usize] = 1;
    let orientations: u16 = if case == 3 { 0xffff } else { block as u16 };
    assert!(m.load(object + 0x112, &orientations.to_le_bytes()));
    assert!(m.load(object + 0x114, &((nodes_count.max(1) * 52) as u16).to_le_bytes()));
    assert!(m.load(object + 0x116, &(matrices as u16).to_le_bytes()));
    assert!(m.load(object + 0x12a, &0x100u16.to_le_bytes()));
    word(m, object + 0x100 + 0x68, if case == 2 { 4 } else { u32::MAX });
    m.bytes[(object + 0x34b) as usize] = if case == 5 { 2 } else { 0 };
    for n in 0..8u32 {
        let q = [0.125 * (n as f32 + 1.0), -0.25, 0.5 - 0.0625 * n as f32, 0.75];
        let rest = [n as f32 * 0.5, -1.5, 2.25, 1.0 + n as f32 / 16.0];
        for (i, v) in q.into_iter().chain(rest).enumerate() {
            word(m, object + block + 32*n + 4*i as u32, v.to_bits());
        }
        let parent_node: u16 = if n == 0 { 0xffff } else { [0, n as u16 - 1][(n % 2) as usize] };
        assert!(m.load(nodes + 0x60*n + 4, &parent_node.to_le_bytes()));
    }
    word(m, parent + 4, 0x400);
    assert!(m.load(parent + 0x116, &0x400u16.to_le_bytes()));
    m.bytes[definition as usize] = if case >= 2 && case != 3 && case != 4 { 0 } else { 1 };
    word(m, definition + 0x38, if case == 4 { u32::MAX } else { 2 });
    word(m, model + 4, 3);
    word(m, skeleton + 0x48, nodes_count);
    word(m, skeleton + 0x4c, nodes);
    // Root transform the faked callee would have produced.
    for i in 0..13 {
        word(m, STACK - 0xf8 + 0xc4 + 4*i, [1.0f32, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.5, 0.25, -2.0][i as usize].to_bits());
    }
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

/// Guest leaves a port runs in Rust. The guest side steps through them, so
/// the next boundary is the port's own next call or return.
const INLINED: &[(u32, &[u32])] = &[(0xbd090, &[0x1421f0, 0x142a60])];

fn inlined(point: u32) -> &'static [u32] {
    INLINED.iter().find(|(p, _)| *p == point).map_or(&[], |(_, leaves)| leaves)
}

fn segment(insns: &BTreeMap<u32, ir::IrInsn>, inline: &[u32], cpu: &mut Cpu, mem: &mut FlatMemory) -> TransferAbi {
    let mut depth = 0u32;
    for _ in 0..500_000 {
        let pc = cpu.eip;
        let i = insns.get(&pc).unwrap_or_else(|| panic!("missing instruction at {pc:#x}"));
        let flow = interp::step(cpu, mem, i).unwrap_or_else(|e| panic!("{pc:#x}: {e:?}"));
        cpu.eip = flow.target();
        match flow {
            Flow::Return { .. } if depth > 0 => depth -= 1,
            Flow::Call { target, .. } if inline.contains(&target) => depth += 1,
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
        assert!(engine::sim_physics::supported(port, &regs, &native), "fixture takes fallback at {at:#x}");
        let actual = port.invoke(&mut regs, &mut native, at).unwrap();
        assert_ne!(actual.route,abi::RESUME,"{} case {case} escaped to generated code",port.switch);
        let expected = segment(insns, inlined(port.point.base), &mut cpu, &mut mem);
        let why = || format!("{} case {case} boundary {n} from {at:#x}", port.switch);
        assert_eq!(actual, expected, "{}", why());
        regs.eip = actual.target_pc;
        assert_eq!(regs, abi_cpu(&cpu), "{}", why());
        let ram = &native.physical_ram()[start..start + RAM];
        if mem.bytes != ram {
            let first = mem.bytes.iter().zip(ram).position(|(a,b)| a != b).unwrap();
            panic!("{} first different byte at {first:#x}", why());
        }
        if actual.kind == abi::RETURN {
            assert_eq!(actual.target_pc, EXIT);
            return;
        }
        assert!(port.resumes.contains(&actual.resume_pc), "unregistered return in {}", why());
        let target = expected.target_pc;
        let cleanup = match target {
            0xbc470 | 0xbc820 | 0x109580 | 0xc49b0 | 0xc58f0 | 0xc60c0 => 4,
            0x23a160 | 0x108c60 | 0xd5de0 | 0xbe8e0 | 0xbc5e0 | 0xbd090 | 0xbdef0 => 4,
            0x1d3920 | 0xbab40 | 0xbdb60 => 12,
            0xe1670 | 0xbf5a0 | 0xbfc90 | 0xbff10 | 0xbfe80 | 0xbfed0 | 0xbfe20 | 0xa98b0 | 0xa9940 => 0,
            0xb8bd0 | 0x1090d0 => 4, 0x16d940 => 8, 0xbdc40 => 20,
            SOLVER_CHILD | 0x131c20 | 0x31c7d0 => 0,
            0x31c520 | 0x31c530 => 4,
            0xbff60 | 0x168311 | 0x1cc260 | 0x166c39 | 0x1469f0 => 4,
            0x16674e | 0x1cb5b0 | 0x1cb520 | 0x259d0 | 0x16760c=>8,
            0x1cb710=>20,0x166992=>12,
            0x155760|0x24590|0xc8a10|0xd03b0|0x1c63f0|0x1c6ea0|0x373b6a|
            0x187420|0x187a40|0x13cb40|0x16840e|0x1d9320=>0,
            0xb6780|0x1c6440|0x1d9370|0xb6760|0x1416c0=>0,
            0x1cd590=>28,0xbd970|0x1c7100=>24,0x1c6f60=>16,0x1cbd80=>12,
            0x1cb490|0x1cc2b0=>8,0x1d9240|0x1c6560|0x100b40|0x1cbd50=>4,
            SUPPORT_CHILD | 0x303360 => 12, SUPPORT_QUERY => 8,
            0x3037a0=>24,0x308590 | 0x303540=>0,0x303dd0=>16,
            0xc7300=>20, 0xc75d0 | 0xc7750=>4, 0x30bf0 | 0x322bd8=>0,
            0x2e96a0 | WELD_REJECT => 16, 0x2f9c10 => 4,
            0x301a40 | WELD_TYPE | WELD_ACCEPT | 0x2f9e80 | 0x302c70 | 0x302eb0 | 0x2f9ca0 => 0,
            WELD_SHAPE | 0x2da730 => 8, WELD_QUERY | 0x2fa0d0 => 20,
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
        if matches!(target,SUPPORT_CHILD|SUPPORT_QUERY) {
            support_vertices(&mut mem,&mut native,&cpu,target);
        }
        if target==0x2f9c10 {
            let frame=sp+8;
            let count=match case {1=>1,2=>5,3=>2,_=>0};
            for (a,v) in [(frame+272,CONTACT_STORAGE),(frame+276,count)] {
                word(&mut mem,a,v);native.set_u32(a,v).unwrap();
            }
        }
        if target==0x2f9e80 {
            let a=cpu.gpr[1]+4;let count=mem.read_u32(a).unwrap()+1;
            word(&mut mem,a,count);native.set_u32(a,count).unwrap();
        }
        if target==WELD_QUERY {
            let a=mem.read_u32(sp+4).unwrap();
            mem.bytes[a as usize]=1;native.set_u8(a,1).unwrap();
        }
        if target == 0x1d3920 && port.point.base == points::OBJECT_NODE_MATRICES.base {
            // Mark nodes whose matrices the component supplies.
            let mask = mem.read_u32(sp + 8).unwrap();
            let bits = [0, 0b0110, 0b1, 0b1010, 0, 0][case as usize % 6];
            word(&mut mem, mask, bits);
            native.set_u32(mask, bits).unwrap();
        }
        if target == 0xe1670 {cpu.xmm[0]=u128::from(1.25f32.to_bits());}
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
        if matches!(target,0x24590|0x259d0) {
            let mut x=x87::abi::X87Abi { st:cpu.x87.st, status:cpu.x87.status, tag:cpu.x87.tag,
                control:cpu.x87.control,..Default::default() };
            let rc=x87::abi::ld_f32(&mut x,(case as f32*0.25).to_bits());
            assert_eq!(rc & x87::abi::status::MASK,x87::abi::status::OK);
            cpu.x87.st=x.st;cpu.x87.status=x.status;cpu.x87.tag=x.tag;
        }
        if target==0x187420 {cpu.xmm[0]=u128::from((case as f32*0.1).to_bits());}
        if target==0x1d9370 {cpu.xmm[0]=u128::from(0.75f32.to_bits());}
        let answer = target.wrapping_mul(0x9127).rotate_left(n % 32) ^ case;
        cpu.gpr[0] = match target {
            0x31c520 | 0x31c530 => cpu.gpr[1] + mem.read_u32(sp+4).unwrap()
                * if target == 0x31c520 {32} else {48},
            0x155760=>0,0x1cb710=>1,0x1cc2b0=>0,0xc8a10=>case%2,0xd03b0=>0,0x187a40=>65535,
            0xb6780=>u32::from(case>=2),0x1c6440=>DATA+0x12000,
            0x1cb490=>u32::from(case>=4),0xb6760=>u32::from(case!=3),
            0x3037a0=>1,0x303540=>u32::MAX,0x303360=>if case==3 {0} else {7},0x308590=>if case==2 {5} else {0},
            0xc7750=>u32::from(case<2),
            WELD_SHAPE=>DATA+1024, WELD_ACCEPT=>DATA+2560, 0x301a40=>0,
            0x2fa0d0=>mem.read_u32(sp+16).unwrap(),
            0x16898b => 0, 0x1cbd30 => case%4,
            0xbaf80 => 1,
            0x120850 => case % 3,
            0xbab40 => n % 2,
            0x14a5b0 => 2,
            0xb8940 | 0xb8a10 => 0x8100_0000,
            _ => 0x006e_1000 | (answer & 0xff),
        };
        // Preserve both volatile pointers for leaf callbacks. The unit and
        // object walks also get arbitrary caller-saved register clobbers.
        if [points::UNIT_UPDATE.base,points::OBJECTS_PRE_PHYSICS.base,
            points::OBJECTS_POST_PHYSICS.base,points::OBJECT_PRE_UPDATE.base,
            points::OBJECT_POST_UPDATE.base,points::OBJECT_NODE_MATRICES.base].contains(&port.point.base) {
            cpu.gpr[1] = answer.rotate_left(9);
            cpu.gpr[2] = !answer;
        }
        cpu.gpr[4] = sp + 4 + cleanup;
        cpu.eip = next;
        regs = abi_cpu(&cpu);
    }
    panic!("unbounded callback loop");
}

struct Oracle {
    image: FlatMemory,
    functions: BTreeMap<u32,BTreeMap<u32,ir::IrInsn>>,
}

fn oracle() -> Option<&'static Oracle> {
    static ORACLE: std::sync::OnceLock<Option<Oracle>> = std::sync::OnceLock::new();
    ORACLE.get_or_init(|| {
        let Some(path) = common::xbe_path() else {
            assert!(std::env::var_os("H2_XBE").is_none(), "H2_XBE does not name a readable file");
            eprintln!("SKIPPED native simulation differential tests: set H2_XBE");
            return None;
        };
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(common::sha256(&bytes), common::IMAGE_SHA256);
        let xbe = xbe::Xbe::parse(&bytes).unwrap();
        let analysis = analyze::analyze(&xbe).unwrap();
        let lifter = lift::Lifter::new(&analysis, &xbe).unwrap();
        let mut image = lift::harness::load_image(&xbe).unwrap();
        image.bytes.truncate(RAM);
        let functions = PORTS.iter().map(|port| {
            let function = lifter.lift(port.point.base).expect(port.point.name);
            let mut insns: BTreeMap<_,_> = function.blocks.into_values().flat_map(|b| b.insns)
                .map(|i| (i.pc,i)).collect();
            if port.point.base == points::HAVOK_WELD_CONTACTS.base {
                // Discovery gives the shared contact tail its own entry.
                // Follow it as the guest would between external calls.
                const CONTACT_TAIL: u32 = 0x002f_a64e;
                let tail = lifter.lift(CONTACT_TAIL).expect("contact tail");
                for i in tail.blocks.into_values().flat_map(|b| b.insns) {
                    insns.entry(i.pc).or_insert(i);
                }
            }
            for &leaf in inlined(port.point.base) {
                let leaf = lifter.lift(leaf).expect("inlined leaf");
                for i in leaf.blocks.into_values().flat_map(|b| b.insns) {
                    insns.entry(i.pc).or_insert(i);
                }
            }
            (port.point.base,insns)
        }).collect();
        Some(Oracle {image,functions})
    }).as_ref()
}

type Fixture = fn(&mut FlatMemory, &mut Cpu, u32);

fn matches_guest(point: &points::Point, uses_objects: bool, cases: u32, prepare: Fixture) {
    let Some(oracle) = oracle() else { return };
    let port = PORTS.iter().find(|p| p.point.base == point.base).unwrap();
    let insns = &oracle.functions[&point.base];
    for case in 0..cases {
        let mut memory = oracle.image.clone();
        word(&mut memory, STACK, EXIT);
        if uses_objects { fixture(&mut memory, case); }
        let mut cpu = Cpu {
            gpr: [0x1234_5678, 0x9876_5432, 0xfe3420, 0x7b1c, STACK, 0xaabbcc, 0x5151, 0xe37],
            eip: point.base,
            eflags: 0xa93,
            ..Default::default()
        };
        // Distinguish MOVSS from memory (clears high lanes) and arithmetic
        // on a register (retains them). Input scalar lanes start at zero.
        for (i, reg) in cpu.xmm.iter_mut().enumerate() {
            *reg = (0x1234_u128 + i as u128) << 100;
        }
        cpu.x87.control = [0x007f,0x027f,0x037f][case as usize % 3];
        cpu.mxcsr |= (case % 4) << 13;
        prepare(&mut memory, &mut cpu, case);
        compare(port, insns, memory, cpu, case);
    }
}

macro_rules! differential {
    ($name:ident, $point:ident, $objects:literal, $cases:literal, $prepare:expr) => {
        #[test]
        fn $name() { matches_guest(&points::$point, $objects, $cases, $prepare); }
    };
}

differential!(havok_solver, HAVOK_SOLVER, false, 23, solver_fixture);
differential!(havok_support_vertices, HAVOK_SUPPORT_VERTICES, false, 6, support_fixture);
differential!(havok_weld_contacts, HAVOK_WELD_CONTACTS, false, 6, weld_fixture);
differential!(objects_pre_physics, OBJECTS_PRE_PHYSICS, true, 6, |_,_,_| {});
differential!(objects_post_physics, OBJECTS_POST_PHYSICS, true, 6, |_,_,_| {});
differential!(object_pre_update, OBJECT_PRE_UPDATE, true, 6, |m,_,n| object_fixture(m,n));
differential!(object_post_update, OBJECT_POST_UPDATE, true, 6, |m,_,n| object_fixture(m,n));
differential!(object_node_matrices, OBJECT_NODE_MATRICES, false, 6, node_fixture);
differential!(object_change_colors, OBJECT_CHANGE_COLORS, true, 6, |m,_,n| color_fixture(m,n));
differential!(object_spatial_visit, OBJECT_SPATIAL_VISIT, false, 6, |m,_,n| visit_fixture(m,n));
differential!(object_spatial_portal, OBJECT_SPATIAL_PORTAL, false, 6, portal_fixture);
differential!(unit_aim_update, UNIT_AIM_UPDATE, false, 6, aim_fixture);
differential!(unit_update, UNIT_UPDATE, true, 6, |_,_,_| {});
differential!(light_record_update, LIGHT_RECORD_UPDATE, true, 6, |m,_,n| light_fixture(m,n));
differential!(first_person_pose_update, FIRST_PERSON_POSE_UPDATE, true, 6, |m,_,n| first_person_fixture(m,n));
differential!(first_person_filter_update, FIRST_PERSON_FILTER_UPDATE, true, 6, |m,_,n| first_person_fixture(m,n));
differential!(observer_camera, OBSERVER_RESULT, false, 6, observer_fixture);
