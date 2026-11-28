//! Halo 1's focused-prop awareness and combat levels, evaluated from native
//! perception records before Halo 2 classifies the actor's best interest.

use guest_layout::{DatumHandle, memory::{GuestMemory, MemFault}, view::objects};

use super::{NONE, actors, preference, state};

pub(super) const FOCUS: u32 = 0x338;
const TRACKER_PROPERTIES: u32 = 0x70;

#[derive(Clone, Copy)]
enum Prop {
    Forgotten,
    Neutral,
    Hostile {
        suppressed: bool,
        threatened: bool,
        recognition: u8,
        vision_result: u16,
        strength: i8,
        distance: f32,
    },
    Indirect {
        suppressed: bool,
        unlocated: bool,
    },
    Memory {
        investigated: bool,
    },
}

impl Prop {
    fn awareness(self) -> usize {
        match self {
            Self::Forgotten => 0,
            Self::Neutral => 1,
            Self::Hostile {
                suppressed: true, ..
            }
            | Self::Indirect {
                suppressed: true, ..
            } => 2,
            Self::Indirect { unlocated, .. } => {
                if unlocated {
                    3
                } else {
                    4
                }
            }
            Self::Memory { investigated } => 5 + usize::from(investigated),
            Self::Hostile {
                threatened: true, ..
            } => 11,
            Self::Hostile {
                recognition: 2.., ..
            } => 10,
            Self::Hostile {
                vision_result: 0 | 1,
                strength,
                distance,
                ..
            } => {
                if strength <= 2 && distance < 6.0 {
                    9
                } else {
                    8
                }
            }
            Self::Hostile { .. } => 7,
        }
    }
}

// Halo 1 0x1506c0, table at 0x1f025c.
const AWARENESS_TO_COMBAT: [u16; 12] = [0, 0, 0, 1, 2, 2, 3, 4, 5, 5, 7, 7];

#[derive(Clone, Copy)]
pub(super) struct Contact {
    pub level: u16,
    pub recognized_at: Option<u32>,
    pub alarm: bool,
}

impl Contact {
    // 0x13db0c, using the selected prop and the actor's own alert mode and baseline.
    pub fn starts_timer(self, alert: bool, baseline: u16) -> bool {
        alert && self.level > baseline
    }

    pub fn releases_timer(self) -> bool {
        self.level >= 5
    }
}

pub(super) fn enabled(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    Ok(state::perception_global(m)?.is_some())
}

fn native_prop(
    m: &dyn GuestMemory,
    interest: u32,
    props: u32,
    depleted: bool,
) -> Result<Prop, MemFault> {
    let kind = m.u16(interest + 0x24)?;
    if kind == 0 {
        return Ok(Prop::Forgotten);
    }
    if !matches!(m.u16(interest + 0x18)?, 1 | 6) {
        return Ok(Prop::Neutral);
    }
    let recognition = m.u8(interest + 0x27)?;
    let threatened = m.u8(props + 0x2a)? != 0;
    let magic = m.u16(interest + 0x1c)? == 3;
    if recognition != 0 || matches!(kind, 1 | 2 | 4) || threatened || magic {
        return Ok(Prop::Hostile {
            suppressed: depleted,
            threatened,
            recognition,
            vision_result: if recognition != 0 {
                m.u16(props + 6)?
            } else {
                4
            },
            strength: m.u8(props + 0x39)? as i8,
            distance: m.f32(interest + 0x28)?,
        });
    }
    if m.u32(props + 0x10)? != NONE {
        return Ok(Prop::Memory {
            investigated: m.u8(props + 0x88)? == 0,
        });
    }
    Ok(Prop::Indirect {
        suppressed: depleted,
        unlocated: m.u8(props + 0x88)? != 0,
    })
}

pub(super) fn focused(
    m: &dyn GuestMemory,
    actor: actors::Actor,
    rate: u32,
) -> Result<Contact, MemFault> {
    let empty = Contact {
        level: 0,
        recognized_at: None,
        alarm: false,
    };
    let Some(interest) = preference::datum(m, preference::INTEREST, m.u32(actor.at + FOCUS)?)?
    else {
        return Ok(empty);
    };
    let Some(tracker) = preference::datum(m, preference::TRACKING, m.u32(interest + 0x14)?)? else {
        return Ok(empty);
    };
    let props = tracker + TRACKER_PROPERTIES;
    let depleted = if let Some((_, object)) = objects::get(m, DatumHandle(m.u32(interest + 0x20)?))?
    {
        object.damage_flags(m)? & 4 != 0
    } else {
        false
    };
    let prop = native_prop(m, interest, props, depleted)?;
    let seen = m.u32(props + 0x10)?;
    // Both engines store the most recent recognized sample (Halo 1
    // 0x15582e, Halo 2 0x265006).
    let recognized_at = (seen != NONE
        && matches!(
            prop,
            Prop::Hostile {
                recognition: 1..,
                ..
            }
        ))
    .then(|| state::authored_tick(seen, rate));
    let alarm = match prop {
        Prop::Hostile {
            suppressed: false,
            recognition,
            threatened,
            ..
        } => recognition != 0 || threatened,
        Prop::Indirect {
            suppressed: false,
            unlocated,
        } => !unlocated,
        _ => false,
    };
    Ok(Contact {
        level: AWARENESS_TO_COMBAT[prop.awareness()],
        recognized_at,
        alarm,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn switches_use_the_focused_props_combat_level() {
        let i = 0x1000;
        let p = 0x2000;
        let mut m = FakeGuest::new();
        m.map(i, 0x3c)
            .map(p, 0xa4)
            .put_u16(i + 0x18, 1)
            .put_u16(i + 0x24, 3)
            .put_u32(p + 0x10, NONE);
        let value = |m: &FakeGuest, depleted| {
            let prop = native_prop(m, i, p, depleted).unwrap();
            Contact {
                level: AWARENESS_TO_COMBAT[prop.awareness()],
                recognized_at: None,
                alarm: false,
            }
        };
        m.put_u8(p + 0x88, 1);
        assert!(!value(&m, false).starts_timer(true, 2));
        assert!(value(&m, false).starts_timer(true, 0));
        assert!(!value(&m, false).starts_timer(false, 0));
        m.put_u16(i + 0x1c, 3).put_u16(p + 6, 4);
        assert_eq!(value(&m, false).level, 4);
        assert!(value(&m, false).starts_timer(true, 2));
        assert!(!value(&m, false).releases_timer());
        // Recognition from an indirect contact is a real sighting.
        m.put_u8(i + 0x27, 1).put_u16(p + 6, 1);
        assert_eq!(value(&m, false).level, 5);
        assert!(value(&m, false).releases_timer());
        m.put_u8(i + 0x27, 2);
        assert_eq!(value(&m, false).level, 7);
        assert_eq!(value(&m, true).level, 0);
        m.put_u8(i + 0x27, 0)
            .put_u16(i + 0x1c, 0)
            .put_u16(i + 0x24, 4);
        assert_eq!(value(&m, false).level, 4);
        assert!(!value(&m, false).releases_timer());
        m.put_u16(i + 0x18, 2);
        assert_eq!(value(&m, false).level, 0);
    }

    #[test]
    fn source_awareness_distinguishes_sample_strength_and_six_world_units() {
        let sight = |distance, strength| {
            Prop::Hostile {
                suppressed: false,
                threatened: false,
                recognition: 1,
                vision_result: 1,
                strength,
                distance,
            }
            .awareness()
        };
        assert_eq!(sight(5.999, 2), 9);
        assert_eq!(sight(6.0, 2), 8);
        assert_eq!(sight(5.0, 3), 8);
    }
}
