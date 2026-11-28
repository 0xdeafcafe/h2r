//! Multiplayer discovery metadata from the hosted squad's lobby selection.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use guest_layout::points;
use live_hle::Hle;
use live_proto::{
    hopper::variant::{GameType, SIZE, VariantRecord},
    session::{BrowserMetadata, MapCategory},
};
use settings::Store;

use crate::{clankers, coop_probe, layout_glue::Gm, map_select};

fn map_category(selected: map_select::Category, packaged: bool) -> Option<MapCategory> {
    use map_select::Category;
    match (selected, packaged) {
        (Category::Halo1, _) => Some(MapCategory::HaloCe),
        (Category::Mods, _) => Some(MapCategory::Mods),
        (Category::Halo2, false) => Some(MapCategory::Halo2),
        (Category::MapPack, false) => Some(MapCategory::MapPack),
        // A package over a retail slot may consist of the base alone.
        // Live can classify its MapId from the manifest in its catalogue.
        (Category::Halo2 | Category::MapPack, true) => None,
    }
}

fn metadata(guest: &impl Gm, packaged: bool, with_clankers: bool) -> Option<BrowserMetadata> {
    let session = coop_probe::read_session(guest)?;
    if !session.live()
        || !session.is_host()
        || session.is_campaign()
        || guest.u32(
            session
                .addr
                .checked_add(crate::variant_ext::SESSION_NETWORK_CLASS)?,
        ) != Some(2)
    {
        return None;
    }

    let mut raw = [0; SIZE];
    let variant = guest
        .rd(
            session
                .addr
                .checked_add(bots::phantom::session::at::VARIANT)?,
            &mut raw,
        )
        .then(|| VariantRecord::from_bytes(&raw))
        .flatten();
    let game_type = variant
        .as_ref()
        .map(VariantRecord::engine)
        .filter(|engine| GameType::from_engine(*engine).is_some());
    let variant = variant
        .filter(|_| game_type.is_some())
        .map(|record| record.name());
    let variant = variant
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        .map(str::to_owned);
    Some(BrowserMetadata {
        clankers: Some(with_clankers),
        game_type,
        variant,
        map_category: map_select::selected_category(session.map)
            .or_else(|| {
                crate::mod_slots::selected_path(guest, session.map)
                    .map(|path| map_select::category_of(&path))
            })
            .and_then(|category| map_category(category, packaged)),
    })
}

struct Advertisements(Arc<Hle>);

impl hooks::Feature for Advertisements {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("live-browser-ads", "Custom Game Browser advertisements")
            .doc("docs/live/live-custom-games.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let live = self.0.clone();
        let tick = AtomicU32::default();
        // The guest need not republish for a pure host-side setting change.
        // Re-read periodically and send only when the advertisement differs.
        b.raw().observe_after(
            &points::GAME_TICK,
            "refresh Live lobby metadata",
            move |o| {
                if tick.fetch_add(1, Ordering::Relaxed) % 15 == 0 {
                    live.lobby_browser_changed(&o.guest());
                }
            },
        );
        Ok(())
    }
}

pub fn install(
    features: &mut hooks::Registry,
    live: &Arc<Hle>,
    store: Option<&Arc<Store>>,
    bots: bool,
) -> anyhow::Result<()> {
    if !features
        .add(&Advertisements(live.clone()))
        .map_err(|error| anyhow::anyhow!("Live browser advertisements: {error}"))?
    {
        return Ok(());
    }

    // The probe runs with guest memory held. A setting handle can be
    // sampled without taking Store's lock.
    let clankers = store
        .filter(|_| bots)
        .map(|store| store.handle::<bool>(clankers::ENABLED.key));
    live.set_browser_probe(Arc::new(move |guest, map_id| {
        let enabled = clankers.as_ref().is_some_and(|setting| setting.get());
        metadata(guest, map_id.is_some(), enabled)
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coop_probe::{NO_DIFFICULTY, layout};
    use runtime::{Guest, guest::Permissions};

    #[test]
    fn ad_uses_the_hosted_live_variant_and_clanker_setting() {
        let mut guest = Guest::new();
        let addr = 0x0100_0000;
        guest
            .map_zeroed(points::NETWORK_READY.base & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        guest.map_zeroed(addr, 0x8000, Permissions::RW).unwrap();
        guest.set_u8(points::NETWORK_READY.base, 1).unwrap();
        guest.set_u32(points::SQUAD_SESSION.base, addr).unwrap();
        guest.set_u32(addr + layout::STATE, 5).unwrap();
        guest.set_u32(addr + layout::MAP, u32::MAX).unwrap();
        guest
            .set_u32(addr + crate::variant_ext::SESSION_NETWORK_CLASS, 2)
            .unwrap();
        guest
            .set_u16(addr + layout::DIFFICULTY, NO_DIFFICULTY)
            .unwrap();
        let variant = VariantRecord::stock(GameType::Ctf, "One Flag Night");
        guest
            .write_bytes(
                addr + bots::phantom::session::at::VARIANT,
                variant.bytes(),
                1,
            )
            .unwrap();

        assert_eq!(
            metadata(&guest, false, true),
            Some(BrowserMetadata {
                clankers: Some(true),
                game_type: Some(GameType::Ctf.engine()),
                variant: Some("One Flag Night".into()),
                map_category: None,
            })
        );
        guest.set_u32(addr + layout::HOST_PEER, 1).unwrap();
        assert_eq!(metadata(&guest, false, true), None);
        guest.set_u32(addr + layout::HOST_PEER, 0).unwrap();
        guest.set_u16(addr + layout::DIFFICULTY, 1).unwrap();
        assert_eq!(metadata(&guest, false, true), None);
    }

    #[test]
    fn the_host_leaves_packaged_retail_slots_for_live_to_classify() {
        use map_select::Category;
        assert_eq!(
            map_category(Category::Halo2, false),
            Some(MapCategory::Halo2)
        );
        assert_eq!(
            map_category(Category::MapPack, false),
            Some(MapCategory::MapPack)
        );
        assert_eq!(map_category(Category::Halo2, true), None);
        assert_eq!(map_category(Category::MapPack, true), None);
        assert_eq!(map_category(Category::Mods, true), Some(MapCategory::Mods));
        assert_eq!(
            map_category(Category::Halo1, true),
            Some(MapCategory::HaloCe)
        );
    }
}
