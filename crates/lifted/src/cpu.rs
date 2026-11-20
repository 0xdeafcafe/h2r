//! The runtime's register file ([`CpuAbi`]) and the interpreter's
//! ([`Cpu`]) are the same state in two shapes. Both directions are exact:
//! nothing is masked or added, so a round trip is the identity on every
//! field the interpreter has.

use ir::interp::{Cpu, X87};
use runtime::CpuAbi;
use runtime::cpu::X87Abi;

/// `c` as the interpreter's state. The time-stamp counter starts at zero.
pub fn to_interp(c: &CpuAbi) -> Cpu {
    Cpu {
        gpr: [c.eax, c.ecx, c.edx, c.ebx, c.esp, c.ebp, c.esi, c.edi],
        eip: c.eip,
        eflags: c.eflags,
        flags_valid: c.eflags_defined,
        fs_base: c.fs_base,
        xmm: c.xmm.map(u128::from_le_bytes),
        x87: X87 {
            st: c.x87.st,
            control: c.x87.control,
            status: c.x87.status,
            tag: c.x87.tag,
            opcode: c.x87.opcode,
            instruction_pointer: c.x87.instruction_pointer,
            data_pointer: c.x87.data_pointer,
            instruction_selector: c.x87.instruction_selector,
            data_selector: c.x87.data_selector,
        },
        mxcsr: c.mxcsr,
        tsc: 0,
    }
}

/// `c` as the runtime's state, with the ABI header filled in.
pub fn from_interp(c: &Cpu) -> CpuAbi {
    let [eax, ecx, edx, ebx, esp, ebp, esi, edi] = c.gpr;
    let x = &c.x87;
    CpuAbi {
        eax,
        ecx,
        edx,
        ebx,
        esp,
        ebp,
        esi,
        edi,
        eip: c.eip,
        fs_base: c.fs_base,
        eflags: c.eflags,
        eflags_defined: c.flags_valid,
        mxcsr: c.mxcsr,
        xmm: c.xmm.map(u128::to_le_bytes),
        x87: X87Abi {
            st: x.st,
            control: x.control,
            status: x.status,
            tag: x.tag,
            opcode: x.opcode,
            instruction_pointer: x.instruction_pointer,
            data_pointer: x.data_pointer,
            instruction_selector: x.instruction_selector,
            data_selector: x.data_selector,
        },
        ..CpuAbi::default()
    }
}

/// One register that differs, with both values as hex.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RegisterDiff {
    pub name: String,
    pub port: String,
    pub original: String,
}

/// Every architectural field where `port` and `original` differ, in a
/// fixed order. EFLAGS compares the defined bits (`flags_valid`, which is
/// compared too); the time-stamp counter is not compared.
pub fn diff(port: &Cpu, original: &Cpu) -> Vec<RegisterDiff> {
    const GPR: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
    let mut out = Vec::new();
    let mut field = |name: &str, a: String, b: String| {
        if a != b {
            out.push(RegisterDiff {
                name: name.into(),
                port: a,
                original: b,
            });
        }
    };
    for (i, name) in GPR.iter().enumerate() {
        field(
            name,
            format!("{:#010x}", port.gpr[i]),
            format!("{:#010x}", original.gpr[i]),
        );
    }
    field(
        "eip",
        format!("{:#010x}", port.eip),
        format!("{:#010x}", original.eip),
    );
    field(
        "eflags_defined",
        format!("{:#010x}", port.flags_valid),
        format!("{:#010x}", original.flags_valid),
    );
    let defined = port.flags_valid & original.flags_valid;
    field(
        "eflags",
        format!("{:#010x}", port.eflags & defined),
        format!("{:#010x}", original.eflags & defined),
    );
    field(
        "fs_base",
        format!("{:#010x}", port.fs_base),
        format!("{:#010x}", original.fs_base),
    );
    field(
        "mxcsr",
        format!("{:#010x}", port.mxcsr),
        format!("{:#010x}", original.mxcsr),
    );
    for i in 0..8 {
        field(
            &format!("xmm{i}"),
            format!("{:#034x}", port.xmm[i]),
            format!("{:#034x}", original.xmm[i]),
        );
    }
    let hex80 = |b: &[u8; 10]| {
        b.iter()
            .rev()
            .map(|x| format!("{x:02x}"))
            .collect::<String>()
    };
    for i in 0..8 {
        field(
            &format!("st{i}"),
            hex80(&port.x87.st[i]),
            hex80(&original.x87.st[i]),
        );
    }
    let (p, o) = (&port.x87, &original.x87);
    for (name, a, b) in [
        ("fpu_control", p.control, o.control),
        ("fpu_status", p.status, o.status),
        ("fpu_tag", p.tag, o.tag),
        ("fpu_opcode", p.opcode, o.opcode),
        ("fpu_cs", p.instruction_selector, o.instruction_selector),
        ("fpu_ds", p.data_selector, o.data_selector),
    ] {
        field(name, format!("{a:#06x}"), format!("{b:#06x}"));
    }
    for (name, a, b) in [
        ("fpu_ip", p.instruction_pointer, o.instruction_pointer),
        ("fpu_dp", p.data_pointer, o.data_pointer),
    ] {
        field(name, format!("{a:#010x}"), format!("{b:#010x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_every_field() {
        let mut c = CpuAbi {
            eax: 1,
            ecx: 2,
            edx: 3,
            ebx: 4,
            esp: 5,
            ebp: 6,
            esi: 7,
            edi: 8,
            eip: 0x1234,
            fs_base: 0x40,
            eflags: 0x893,
            eflags_defined: 0x8d5,
            mxcsr: 0x9fa0,
            ..CpuAbi::default()
        };
        c.xmm[3] = [7; 16];
        c.x87.st[2] = [9; 10];
        c.x87.status = 0x3800;
        c.x87.data_pointer = 0x55;
        assert_eq!(from_interp(&to_interp(&c)), c);
    }

    #[test]
    fn diff_names_only_what_differs() {
        let a = to_interp(&CpuAbi::default());
        let mut b = a.clone();
        assert!(diff(&a, &b).is_empty());
        b.gpr[0] = 9;
        b.xmm[1] = 1;
        b.tsc = 77;
        let names: Vec<_> = diff(&a, &b).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["eax", "xmm1"]);
    }

    #[test]
    fn undefined_flags_are_not_compared() {
        let mut a = to_interp(&CpuAbi::default());
        a.flags_valid = !0x10;
        let mut b = a.clone();
        b.eflags ^= 0x10;
        assert!(diff(&a, &b).is_empty());
        b.eflags ^= 0x1;
        assert_eq!(diff(&a, &b)[0].name, "eflags");
    }
}
