//! Between commands the driver owns a guest squad pointer, its configuration
//! and the acknowledgements observed so far. Guest session bytes are in RAM.
use super::{Config, Machine, Owned, Setup, Variant, variant};
use snapshot::codec::{Reader, Writer};

fn corrupt(reason: &str) -> snapshot::Error {
    snapshot::Error::Corrupt("host-lobby", reason.into())
}

impl Machine {
    pub(in crate::host_lobby) fn save(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(1).bool(self.owned.is_some());
        if let Some(o) = &self.owned {
            w.u32(o.addr)
                .u32(o.config.map)
                .u8(o.config.max_players)
                .bool(o.config.live);
            match &o.config.variant {
                Variant::Stock(t) => {
                    w.u8(0).u32(t.list_index());
                }
                Variant::Named(v) => {
                    w.u8(1).raw(v.bytes());
                }
            }
            w.u8(match o.setup {
                Setup::Created => 0,
                Setup::Applied => 1,
                Setup::Open => 2,
            });
            w.bool(o.applied_variant.is_some());
            if let Some(v) = &o.applied_variant {
                w.raw(v.as_slice());
            }
            for id in [&o.prior_live_id, &o.live_id] {
                w.bool(id.is_some());
                if let Some(id) = id {
                    w.str(id);
                }
            }
            w.bool(o.closing);
        }
        w.into_bytes()
    }

    pub(in crate::host_lobby) fn load(bytes: &[u8]) -> Result<Self, snapshot::Error> {
        let mut r = Reader::new(bytes, "host-lobby");
        if r.u8()? != 1 {
            return Err(corrupt("unknown version"));
        }
        let owned = if r.bool()? {
            let addr = r.u32()?;
            if addr == 0 || addr.checked_add(0x7900).is_none() {
                return Err(corrupt("squad pointer"));
            }
            let map = r.u32()?;
            let max_players = r.u8()?;
            if !(1..=16).contains(&max_players) {
                return Err(corrupt("player limit"));
            }
            let live = r.bool()?;
            let variant = match r.u8()? {
                0 => Variant::Stock(
                    *sim::config::GameType::ALL
                        .get(r.u32()? as usize)
                        .ok_or_else(|| corrupt("stock variant"))?,
                ),
                1 => Variant::Named(Box::new(
                    variant::VariantRecord::from_bytes(r.raw(variant::SIZE)?)
                        .ok_or_else(|| corrupt("variant bytes"))?,
                )),
                _ => return Err(corrupt("variant kind")),
            };
            let config = Config {
                map,
                variant,
                max_players,
                live,
            };
            let setup = match r.u8()? {
                0 => Setup::Created,
                1 => Setup::Applied,
                2 => Setup::Open,
                _ => return Err(corrupt("squad setup")),
            };
            let applied_variant = r.bool()?.then(|| r.array().map(Box::new)).transpose()?;
            let mut id = || -> Result<Option<String>, snapshot::Error> {
                r.bool()?.then(|| r.str().map(str::to_owned)).transpose()
            };
            let prior_live_id = id()?;
            let live_id = id()?;
            Some(Owned {
                addr,
                config,
                setup,
                applied_variant,
                prior_live_id,
                live_id,
                closing: r.bool()?,
            })
        } else {
            None
        };
        r.finish()?;
        Ok(Self { owned })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_squad_round_trip_and_incomplete_state() {
        let config = Config {
            map: 99,
            variant: Variant::Stock(sim::config::GameType::King),
            max_players: 16,
            live: true,
        };
        let state = Machine {
            owned: Some(Owned {
                addr: 0x8000,
                config,
                setup: Setup::Open,
                applied_variant: Some(Box::new([27; variant::SIZE])),
                prior_live_id: None,
                live_id: Some("current".into()),
                closing: false,
            }),
        };
        let bytes = state.save();
        assert_eq!(Machine::load(&bytes).unwrap().save(), bytes);
        assert!(Machine::load(&bytes[..bytes.len() - 1]).is_err());
    }
}
