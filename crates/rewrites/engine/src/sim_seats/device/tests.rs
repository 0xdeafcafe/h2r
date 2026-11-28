use super::*;

const ONE: u32 = 0x3f80_0000;

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// The x87 pieces in host single precision: the kernel's shape, not
/// the model's rounding.
struct Host;

impl Fpu for Host {
    fn speed(&mut self, v: [u32; 3], lo: u32, span: u32, _: u32) -> Option<(u32, bool)> {
        let [x, y, z] = v.map(f32::from_bits);
        let r = ((x * x + y * y + z * z).sqrt() - f32::from_bits(lo)) / f32::from_bits(span);
        Some((r.to_bits(), r < 0.0))
    }
    fn pow(&mut self, t: u32, e: u32) -> Option<u32> {
        Some(f32::from_bits(t).powf(f32::from_bits(e)).to_bits())
    }
    fn abs(&mut self, v: u32) -> Option<u32> {
        Some(v & 0x7fff_ffff)
    }
    fn above(&mut self, a: u32, dead: u32) -> Option<bool> {
        Some(f32::from_bits(a) > f32::from_bits(dead))
    }
    fn hypot(&mut self, q: u32, p: u32) -> Option<u32> {
        let (q, p) = (f32::from_bits(q), f32::from_bits(p));
        Some((q * q + p * p).sqrt().to_bits())
    }
}

fn device() -> Device {
    let mut block = [0; 20];
    block[16] = f(0.5);
    block[17] = f(-0.25);
    Device {
        rates: [f(2.0), f(3.0)],
        block,
        seat: None,
        blocked: false,
        wanted: [0; 2],
        k: Constants {
            one: ONE,
            zero: 0,
            scale: f(0.5),
            dead: f(0.1),
            low: f(-1.0),
        },
    }
}

#[test]
fn an_unbound_stick_is_normalised_on_its_larger_axis() {
    let out = read(&device(), &mut Host).unwrap();
    assert_eq!(out.rates, [f(1.0), f(1.5)]);
    let Path::Steer(s) = out.path else {
        panic!("blocked")
    };
    // |0.5| is the larger axis: the ratio 0.5 and its hypot scale both.
    let h = (1.0f32 + 0.25).sqrt();
    assert_eq!(s.hypot, Some(f(h)));
    assert_eq!(s.scaled, [f(h * -0.25), f(h * 0.5)]);
    assert_eq!(s.axes, [f(h * -0.25), f(h * 0.5)]);
}

#[test]
fn a_stick_inside_the_dead_zone_keeps_its_scale() {
    let mut d = device();
    d.block[17] = f(0.05);
    d.block[16] = f(4.0);
    let Path::Steer(s) = read(&d, &mut Host).unwrap().path else {
        panic!("blocked")
    };
    assert_eq!(s.hypot, None);
    assert_eq!(s.scaled, [f(0.05), f(4.0)]);
    // The clamp holds the vertical axis at one.
    assert_eq!(s.axes, [f(0.05), ONE]);
}

#[test]
fn a_vehicle_seat_bends_the_rates_by_its_speed() {
    let mut d = device();
    d.seat = Some(Seat {
        velocity: [f(3.0), f(4.0), 0],
        lo: f(1.0),
        hi: f(9.0),
        exponent: f(2.0),
        yaw: [f(1.0), f(5.0)],
        pitch: [0, 0],
    });
    let out = read(&d, &mut Host).unwrap();
    // (5 - 1) / 8 = 0.5, squared.
    let v = out.vehicle.unwrap();
    assert_eq!(
        (v.lo, v.span, v.t, v.last),
        (f(1.0), f(8.0), f(0.25), Last::Pow)
    );
    // Yaw: (1 + 4 * 0.25) * 0.5; pitch has no positive end and keeps the port's.
    assert_eq!(out.rates, [f(1.0), f(1.5)]);
}

#[test]
fn a_still_vehicle_and_a_zero_exponent_leave_no_pow() {
    let mut d = device();
    d.seat = Some(Seat {
        lo: f(2.0),
        hi: f(2.0),
        exponent: 0,
        yaw: [f(1.0), f(3.0)],
        ..Seat::default()
    });
    let out = read(&d, &mut Host).unwrap();
    let v = out.vehicle.unwrap();
    assert_eq!((v.span, v.t, v.last), (0, 0, Last::None));
    assert_eq!(out.rates[0], f(0.5));
}

#[test]
fn blocked_input_drops_the_wanted_buttons_not_pressed() {
    let mut d = device();
    d.blocked = true;
    // Buttons 0 and 1 wanted in both halves, button 2 in one; only 1 is
    // pressed.
    d.wanted = [0b111, 0b011];
    d.block[0] = 0x0000_0700;
    d.block[4] = 0x0022_0011;
    d.block[5] = 0x0044_0033;
    let Path::Blocked(s) = read(&d, &mut Host).unwrap().path else {
        panic!("steered")
    };
    assert_eq!(s.walked, Some(0b011));
    assert_eq!(s.wanted, [0b110, 0b010]);
    // Button 0 is no longer wanted: its state and hold come over. Buttons
    // 1 and 2 stay wanted and zero.
    assert_eq!(&s.states[..4], &[0, 0, 0, 0]);
    assert_eq!(&s.holds[..4], &[0x11, 0, 0, 0x44]);
}

#[test]
fn an_operand_off_the_exact_range_is_inexact() {
    let mut d = device();
    d.rates[0] = 0x7fc0_0000;
    assert_eq!(read(&d, &mut Host), None);
}

/// The live MXCSR, PE clear: the outcome is the fast path's, and the
/// inexact rate product raises PE.
#[test]
fn the_live_mxcsr_reads_as_the_fast_path_and_raises_pe() {
    let mut d = device();
    d.rates[0] = f(1.1);
    d.k.scale = f(0.3);
    let fast = read(&d, &mut Host).unwrap();
    assert_eq!(fast.rates[0], f(1.1 * 0.3));
    assert_eq!(read_guest(&d, 0x1f80, &mut Host), Some((fast, 0x1fa0)));
    assert_eq!(read_guest(&d, 0x1fa0, &mut Host), Some((fast, 0x1fa0)));
}

/// Off the exact range the read is the SSE model's, as the guest's
/// `mulss` runs it: a quiet NaN rate propagates and raises nothing.
#[test]
fn an_operand_off_the_exact_range_reads_as_the_guest_runs_it() {
    let mut d = device();
    d.rates[0] = 0x7fc0_0000;
    let (out, mxcsr) = read_guest(&d, 0x1fa0, &mut Host).unwrap();
    assert_eq!(out.rates[0], 0x7fc0_0000);
    assert_eq!(mxcsr, 0x1fa0);
}

/// Only an unmasked exception declines: a signalling NaN under IM clear.
#[test]
fn an_unmasked_exception_declines() {
    let mut d = device();
    d.rates[0] = 0x7f80_0001;
    assert_eq!(read_guest(&d, 0x1f00, &mut Host), None);
    assert!(read_guest(&d, 0x1f80, &mut Host).is_some(), "masked, IE is raised and the read goes on");
}

/// A rounding mode off nearest rounds as the MXCSR says.
#[test]
fn the_mxcsr_rounding_mode_rounds_the_rates() {
    let mut d = device();
    d.rates[0] = f(1.1);
    d.k.scale = f(0.3);
    let exact = f64::from(1.1f32) * f64::from(0.3f32);
    let nearest = exact as f32;
    let toward_zero = if f64::from(nearest) > exact { f32::from_bits(nearest.to_bits() - 1) } else { nearest };
    let (out, mxcsr) = read_guest(&d, 0x7f80, &mut Host).unwrap();
    assert_eq!(out.rates[0], toward_zero.to_bits());
    assert_eq!(mxcsr & 0x20, 0x20, "inexact");
}
