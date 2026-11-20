//! Executable SDM rules. Approximation table identity needs a physical PIII.
use crate::{Oracle, State};
use anyhow::Result;
use serde::Serialize;
#[derive(Debug, Serialize)]
pub struct Check {
    pub rule: String,
    pub passed: bool,
    pub actual: String,
    pub expected: String,
}
pub fn check(oracle: &mut Oracle) -> Result<Vec<Check>> {
    let mut checks = Vec::new();
    type Probe<'a> = (&'a str, &'a [u8], u32, u32, u32, u32, u32);
    let probes: &[Probe<'_>] = &[
        (
            "add: invalid default NaN",
            &[0xf3, 0x0f, 0x58, 0xc1],
            0x7f800000,
            0xff800000,
            0x1f80,
            0xffc00000,
            1,
        ),
        (
            "sqrt: negative default NaN",
            &[0xf3, 0x0f, 0x51, 0xc1],
            0,
            0xbf800000,
            0x1f80,
            0xffc00000,
            1,
        ),
        (
            "add: SNaN payload quieted",
            &[0xf3, 0x0f, 0x58, 0xc1],
            0x7f812345,
            0x3f800000,
            0x1f80,
            0x7fc12345,
            1,
        ),
        (
            "add: denormal operand flag",
            &[0xf3, 0x0f, 0x58, 0xc1],
            1,
            0,
            0x1f80,
            1,
            2,
        ),
        (
            "mul: exact denormal FTZ off",
            &[0xf3, 0x0f, 0x59, 0xc1],
            0x00800000,
            0x3f000000,
            0x1f80,
            0x00400000,
            0,
        ),
        (
            "mul: exact tiny result FTZ on",
            &[0xf3, 0x0f, 0x59, 0xc1],
            0x00800000,
            0x3f000000,
            0x9f80,
            0,
            0x30,
        ),
        (
            "add: FTZ preserves denormal input flag",
            &[0xf3, 0x0f, 0x58, 0xc1],
            1,
            0x3f800000,
            0x9f80,
            0x3f800000,
            0x22,
        ),
        (
            "rcp: denormal becomes infinity, no flags",
            &[0xf3, 0x0f, 0x53, 0xc1],
            0,
            1,
            0x1f80,
            0x7f800000,
            0,
        ),
        (
            "rcp: negative zero",
            &[0xf3, 0x0f, 0x53, 0xc1],
            0,
            0x80000000,
            0x1f80,
            0xff800000,
            0,
        ),
        (
            "rcp: SNaN quieted without invalid",
            &[0xf3, 0x0f, 0x53, 0xc1],
            0,
            0x7f812345,
            0x1f80,
            0x7fc12345,
            0,
        ),
        (
            "rsqrt: denormal becomes infinity",
            &[0xf3, 0x0f, 0x52, 0xc1],
            0,
            1,
            0x1f80,
            0x7f800000,
            0,
        ),
        (
            "rsqrt: negative default NaN, no flags",
            &[0xf3, 0x0f, 0x52, 0xc1],
            0,
            0xbf800000,
            0x1f80,
            0xffc00000,
            0,
        ),
    ];
    for &(rule, bytes, a, b, mxcsr, expected, flags) in probes {
        let mut s = State {
            mxcsr,
            ..State::default()
        };
        s.xmm[0][0] = a;
        s.xmm[1][0] = b;
        let out = oracle.query(bytes, &s)?;
        let actual = out.state.map(|s| (s.xmm[0][0], s.mxcsr & 63));
        checks.push(Check {
            rule: rule.into(),
            passed: actual == Some((expected, flags)),
            actual: format!("{actual:08x?}"),
            expected: format!("{expected:08x}, flags={flags:02x}"),
        });
    }
    for rc in 0..4 {
        for ftz in 0..2 {
            let mxcsr = 0x1f80 | rc << 13 | ftz << 15;
            for (name, op, a, b, expected, flags) in [
                (
                    "add: halfway rounding",
                    0x58,
                    0x3f800000,
                    0x33800000,
                    if rc == 2 { 0x3f800001 } else { 0x3f800000 },
                    32,
                ),
                (
                    "sub: cancellation sign",
                    0x5c,
                    0x3f800000,
                    0x3f800000,
                    if rc == 1 { 0x80000000 } else { 0 },
                    0,
                ),
                (
                    "min: equal zeros select source",
                    0x5d,
                    0,
                    0x80000000,
                    0x80000000,
                    0,
                ),
                (
                    "max: SNaN source returned unchanged",
                    0x5f,
                    0x3f800000,
                    0x7f812345,
                    0x7f812345,
                    1,
                ),
                (
                    "min: QNaN destination selects source",
                    0x5d,
                    0x7fc12345,
                    0x3f800000,
                    0x3f800000,
                    1,
                ),
            ] {
                let mut s = State {
                    mxcsr,
                    ..State::default()
                };
                s.xmm[0][0] = a;
                s.xmm[1][0] = b;
                let out = oracle.query(&[0xf3, 0x0f, op, 0xc1], &s)?.state;
                let actual = out.map(|s| (s.xmm[0][0], s.mxcsr & 63));
                checks.push(Check {
                    rule: format!("{name} rc={rc} ftz={ftz}"),
                    passed: actual == Some((expected, flags)),
                    actual: format!("{actual:08x?}"),
                    expected: format!("{expected:08x}, flags={flags:02x}"),
                });
            }
            for (op, label) in [(0x2f, "comiss"), (0x2e, "ucomiss")] {
                for (a, b, eflags, flags) in [
                    (0x3f800000, 0x40000000, 1, 0),
                    (0x40000000, 0x3f800000, 0, 0),
                    (0, 0x80000000, 0x40, 0),
                    (0x7fc12345, 0x3f800000, 0x45, u32::from(op == 0x2f)),
                    (0x7f812345, 0, 0x45, 1),
                    (1, 0, 0, 2),
                ] {
                    let mut s = State {
                        mxcsr,
                        eflags: 0xad7,
                        ..State::default()
                    };
                    s.xmm[0][0] = a;
                    s.xmm[1][0] = b;
                    let actual = oracle
                        .query(&[0x0f, op, 0xc1], &s)?
                        .state
                        .map(|s| (s.eflags & 0x8d5, s.mxcsr & 63));
                    checks.push(Check {
                        rule: format!("{label} {a:08x}/{b:08x} rc={rc} ftz={ftz}"),
                        passed: actual == Some((eflags, flags)),
                        actual: format!("{actual:08x?}"),
                        expected: format!("eflags={eflags:03x}, flags={flags:02x}"),
                    });
                }
            }
            for trunc in [false, true] {
                for bits in [
                    0u32, 0x80000000, 0x3f000000, 0xbf000000, 0x3fc00000, 0xbfc00000, 0x40200000,
                    0xc0200000, 0x4effffff, 0x4f000000, 0x4f000001, 0xceffffff, 0xcf000000,
                    0xcf000001, 0x7fc12345, 0x7f800001, 0x7f800000, 0xff800000,
                ] {
                    let x = f32::from_bits(bits) as f64;
                    let rounded = if trunc || rc == 3 {
                        x.trunc()
                    } else {
                        match rc {
                            0 => x.round_ties_even(),
                            1 => x.floor(),
                            _ => x.ceil(),
                        }
                    };
                    let (expected, flags) = if !rounded.is_finite()
                        || rounded < i32::MIN as f64
                        || rounded > i32::MAX as f64
                    {
                        (0x80000000, 1)
                    } else {
                        (rounded as i32 as u32, if rounded == x { 0 } else { 32 })
                    };
                    let mut s = State {
                        mxcsr,
                        ..State::default()
                    };
                    s.xmm[1][0] = bits;
                    let actual = oracle
                        .query(&[0xf3, 0x0f, if trunc { 0x2c } else { 0x2d }, 0xc1], &s)?
                        .state
                        .map(|s| (s.eax, s.mxcsr & 63));
                    checks.push(Check {
                        rule: format!("cvt{trunc} {bits:08x} rc={rc} ftz={ftz}"),
                        passed: actual == Some((expected, flags)),
                        actual: format!("{actual:08x?}"),
                        expected: format!("{expected:08x}, flags={flags:02x}"),
                    });
                }
            }
        }
    }
    for (op, name) in [(0x53, "rcp"), (0x52, "rsqrt")] {
        let mut max_error = 0f64;
        let mut passed = true;
        let mut fingerprint = Vec::new();
        for n in 0..4096u32 {
            let bits = 0x3f800000 + (n << 12); // [1,4): both exponent parities.
            let x = f32::from_bits(bits) as f64;
            let reference = if op == 0x53 { 1.0 / x } else { 1.0 / x.sqrt() };
            let mut baseline = None;
            for mode in 0..8 {
                let mut s = State {
                    mxcsr: 0x1f80 | (mode & 3) << 13 | (mode >> 2) << 15,
                    ..State::default()
                };
                s.xmm[1][0] = bits;
                let out = oracle.query(&[0xf3, 0x0f, op, 0xc1], &s)?;
                if let Some(s) = out.state {
                    let actual = s.xmm[0][0];
                    let error = ((f32::from_bits(actual) as f64 - reference) / reference).abs();
                    max_error = max_error.max(error);
                    passed &= error <= 1.5 / 4096.0 && s.mxcsr & 63 == 0;
                    passed &= baseline.is_none_or(|b| b == actual);
                    baseline = Some(actual);
                    if mode == 0 && n < 8 {
                        fingerprint.push(format!("{bits:08x}->{actual:08x}"));
                    }
                } else {
                    passed = false;
                }
            }
        }
        checks.push(Check {
            rule: format!("{name}: 4096 inputs x 8 modes, relative error and mode invariance"),
            passed,
            actual: format!("max error={max_error:.12}; {}", fingerprint.join(", ")),
            expected:
                "error <= 1.5/4096, no flags, independent of RC/FTZ; PIII table identity unverified"
                    .into(),
        });
    }
    for loaded in [0x1f80, 0x9f80, 0x3f81, 0x5fa0, 0x7fbf, 0x9fbf] {
        let mut s = State::default();
        s.memory[0] = loaded;
        let out = oracle.query(&[0x0f, 0xae, 0x17], &s)?;
        let actual = out.state.map(|s| s.mxcsr);
        checks.push(Check {
            rule: format!("ldmxcsr valid state {loaded:08x}"),
            passed: actual == Some(loaded),
            actual: format!("{actual:08x?}"),
            expected: format!("{loaded:08x}"),
        });
    }
    for loaded in [0x1fc0u32, 0x0000] {
        let mut s = State::default();
        s.memory[0] = loaded;
        let out = oracle.query(&[0x0f, 0xae, 0x17], &s)?;
        let actual = out.state.map(|s| s.mxcsr);
        let (passed, rule, expected) = if loaded == 0x1fc0 {
            (
                out.exception.is_some(),
                "ldmxcsr: DAZ is reserved on Pentium III",
                "#GP on Pentium III (modern CPUs may support DAZ)".into(),
            )
        } else {
            (
                actual == Some(0),
                "ldmxcsr: clearing exception masks",
                "00000000, no immediate exception".into(),
            )
        };
        checks.push(Check {
            rule: rule.into(),
            passed,
            actual: format!("signal={:?}, mxcsr={actual:08x?}", out.exception),
            expected,
        });
    }
    // Reserved bits must #GP on Intel; translation layers can silently accept them.
    for reserved in [0x00011f80u32, 0x80001f80, 0xffffffff] {
        let mut s = State::default();
        s.memory[0] = reserved;
        let out = oracle.query(&[0x0f, 0xae, 0x17], &s)?;
        checks.push(Check {
            rule: format!("ldmxcsr reserved bits {reserved:08x} must #GP"),
            passed: out.exception.is_some(),
            actual: format!(
                "signal={:?}, mxcsr={:08x?}",
                out.exception,
                out.state.map(|s| s.mxcsr)
            ),
            expected: "#GP (OS signal), no completed state".into(),
        });
    }
    Ok(checks)
}
