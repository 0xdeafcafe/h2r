//! Independent MPFR interval fixtures, generated from synthetic ext80 inputs.
//! No MPFR dependency is required to execute the committed fixtures.
use x87::{F80, Flags, Mode, Pc, Rc, add, cos, patan, sin, sincos, tan, x2m1, yl2x};

#[test]
fn hot_transcendentals_match_independent_mpfr_intervals() {
    let mut count = [0; 6];
    for line in include_str!("data/trans_oracle.txt").lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let hex = |n| u64::from_str_radix(fields[n], 16).unwrap();
        let op: usize = fields[0].parse().unwrap();
        let y = F80::new(hex(2) as u16, hex(1));
        let x = F80::new(hex(4) as u16, hex(3));
        let expected = (F80::new(hex(6) as u16, hex(5)), Flags(hex(7) as u16));
        let trig = |t: x87::Transcendental| {
            assert!(!t.out_of_range, "{line}");
            (t.value, t.flags)
        };
        let actual = match op {
            0 => yl2x(y, x),
            1 => x2m1(x),
            2 => trig(sin(x)),
            3 => trig(cos(x)),
            4 => trig(tan(x)),
            5 => patan(y, x),
            _ => panic!("unknown oracle operation"),
        };
        // FSINCOS returns the same two values (its flags merge both).
        match op {
            2 => assert_eq!(sincos(x).0.value, actual.0, "{line}"),
            3 => assert_eq!(sincos(x).1, actual.0, "{line}"),
            _ => {}
        }
        assert_eq!(actual, expected, "{line}");
        // Transcendentals retain ext80 precision independent of PC. Verify
        // the subsequent PC rounding against independently rounded MPFR data.
        for (pc, at) in [(Pc::Double, 8), (Pc::Single, 11)] {
            let expected_pc = (
                F80::new(hex(at + 1) as u16, hex(at)),
                Flags(hex(at + 2) as u16),
            );
            assert_eq!(
                add(actual.0, F80::ZERO, Mode { pc, rc: Rc::Near }),
                expected_pc,
                "PC={pc:?}: {line}"
            );
        }
        count[op] += 1;
    }
    assert_eq!(count, [512, 512, 256, 256, 256, 256]);
}
