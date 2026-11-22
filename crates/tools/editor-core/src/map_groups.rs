//! The editor's map sections, shared by the map pickers and the agent API.

/// A map's section, in picker order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MapGroup {
    Multiplayer,
    Campaign,
    Other,
    MapPack,
    Packages,
    Halo1Sandbox,
    SharedAndMenu,
}

impl MapGroup {
    /// Apply the reserved cache names even when the map header or menu
    /// catalog is unavailable.
    pub fn classify(stem: &str, kind: Self) -> Self {
        if ["shared", "single_player_shared", "mainmenu", "ui"]
            .iter()
            .any(|name| name.eq_ignore_ascii_case(stem))
        {
            Self::SharedAndMenu
        } else {
            kind
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Multiplayer => "Multiplayer maps",
            Self::Campaign => "Campaign levels",
            Self::Other => "Other maps",
            Self::MapPack => "Map Pack",
            Self::Packages => "Packages",
            Self::Halo1Sandbox => "Halo 1 sandbox variants",
            Self::SharedAndMenu => "Shared and menu",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Multiplayer => "multiplayer",
            Self::Campaign => "campaign",
            Self::Other => "other",
            Self::MapPack => "map_pack",
            Self::Packages => "packages",
            Self::Halo1Sandbox => "halo1_sandbox",
            Self::SharedAndMenu => "shared_and_menu",
        }
    }

    pub fn sort_key(self, label: &str) -> (Self, String) {
        (self, label.to_lowercase())
    }
}

impl From<map::CacheType> for MapGroup {
    fn from(kind: map::CacheType) -> Self {
        match kind {
            map::CacheType::Multiplayer => Self::Multiplayer,
            map::CacheType::Campaign => Self::Campaign,
            map::CacheType::MainMenu | map::CacheType::Shared | map::CacheType::SinglePlayerShared => {
                Self::SharedAndMenu
            }
            map::CacheType::Unknown(_) => Self::Other,
        }
    }
}

impl From<h1map::CacheType> for MapGroup {
    fn from(kind: h1map::CacheType) -> Self {
        match kind {
            h1map::CacheType::Multiplayer => Self::Multiplayer,
            h1map::CacheType::Campaign => Self::Campaign,
            h1map::CacheType::UserInterface => Self::SharedAndMenu,
            h1map::CacheType::Other(_) => Self::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MapGroup::*;

    #[test]
    fn reserved_map_names_are_shared_and_menu_in_any_case() {
        for stem in ["SHARED", "single_player_shared", "MainMenu", "Ui"] {
            assert_eq!(SharedAndMenu, super::MapGroup::classify(stem, Multiplayer));
        }
        assert_eq!(Multiplayer, super::MapGroup::classify("ui_arena", Multiplayer));
    }

    #[test]
    fn headers_classify_unlisted_levels_and_renamed_system_maps() {
        use map::CacheType;
        for (kind, group) in [
            (CacheType::Campaign, Campaign),
            (CacheType::Multiplayer, Multiplayer),
            (CacheType::MainMenu, SharedAndMenu),
            (CacheType::Shared, SharedAndMenu),
            (CacheType::SinglePlayerShared, SharedAndMenu),
            (CacheType::Unknown(17), Other),
        ] {
            assert_eq!(super::MapGroup::classify("custom", kind.into()), group);
        }
        assert_eq!(super::MapGroup::from(h1map::CacheType::UserInterface), SharedAndMenu);
    }
}
