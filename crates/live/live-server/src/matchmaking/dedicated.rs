//! The server-side opt-in and regional QoS targets for ad-hoc hosts.

use super::{random_id, selection::Entrant};
use crate::app::Live;
use live_proto::{
    Xuid,
    hopper::HOPPERS,
    matchmaking::{Candidate, Sample},
};
use serde::Deserialize;
use std::{collections::BTreeMap, net::SocketAddrV4};

const ENABLED: &str = "LIVE_DEDICATED";
const HOPPERS_ENABLED: &str = "LIVE_DEDICATED_HOPPERS";
const REGIONS: &str = "LIVE_HOST_REGIONS";
const DEFAULT_REGION: &str = "LIVE_HOST_DEFAULT_REGION";
// Regional probes use the ordinary QoS wire format. These target IDs
// have their own marker, distinct from the account and bot markers.
const PROBE_XUID_MARKER: u64 = 0x000d_0000_0000_0000;

pub(super) struct DedicatedConfig {
    hoppers: BTreeMap<u32, DedicatedPlaylist>,
    pub(super) regions: RegionTable,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DedicatedPlaylist {
    pub(super) map: String,
    pub(super) variant: Option<String>,
}

impl Default for DedicatedConfig {
    fn default() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        return Self::from_lookup(|key| std::env::var(key).ok()).unwrap_or_else(|err| {
            eprintln!("h2 live: dedicated matchmaking disabled: {err}");
            Self::disabled()
        });

        #[cfg(target_arch = "wasm32")]
        Self::disabled()
    }
}

impl DedicatedConfig {
    fn disabled() -> Self {
        Self {
            hoppers: BTreeMap::new(),
            regions: RegionTable::default(),
        }
    }

    pub(super) fn from_lookup(
        mut lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<Self, String> {
        if !lookup(ENABLED).is_some_and(|value| value.trim() == "1") {
            return Ok(Self::disabled());
        }
        let Some(configured) = lookup(HOPPERS_ENABLED).filter(|value| !value.trim().is_empty())
        else {
            return Ok(Self::disabled());
        };
        let hoppers: BTreeMap<u32, DedicatedPlaylist> = serde_json::from_str(&configured)
            .map_err(|err| format!("{HOPPERS_ENABLED}: {err}"))?;
        for (&index, playlist) in &hoppers {
            if index >= HOPPERS as u32 {
                return Err(format!("{HOPPERS_ENABLED} contains an invalid hopper: {index}"));
            }
            let map = &playlist.map;
            if map.is_empty()
                || !map
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return Err(format!(
                    "{HOPPERS_ENABLED} needs a map scenario name for hopper {index}"
                ));
            }
        }
        let list = lookup(REGIONS)
            .map(|json| {
                serde_json::from_str::<BTreeMap<String, Option<SocketAddrV4>>>(&json)
                    .map_err(|err| format!("{REGIONS}: {err}"))
            })
            .transpose()?
            .unwrap_or_default();
        let default = lookup(DEFAULT_REGION)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        Ok(Self {
            hoppers,
            regions: RegionTable::new(list, default)?,
        })
    }

    pub(super) fn playlist(&self, hopper: u32) -> Option<&DedicatedPlaylist> {
        self.hoppers.get(&hopper)
    }
}

impl Live {
    /// Load the queue's dedicated-host settings from the Worker's bindings.
    pub fn configure_dedicated_matchmaking(
        &self,
        lookup: impl FnMut(&str) -> Option<String>,
    ) -> Result<(), String> {
        let config = DedicatedConfig::from_lookup(lookup)?;
        self.matchmaking.lock().unwrap().config = config;
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct RegionTable {
    regions: BTreeMap<String, Option<Candidate>>,
    default: Option<String>,
}

pub(super) struct RegionChoice<'a> {
    pub(super) region: &'a str,
    pub(super) rtt_ms: BTreeMap<Xuid, u16>,
}

impl RegionTable {
    fn new(
        list: BTreeMap<String, Option<SocketAddrV4>>,
        default: Option<String>,
    ) -> Result<Self, String> {
        match default.as_deref() {
            Some(region) if list.contains_key(region) => {}
            None if list.is_empty() => {}
            _ => return Err(format!("{DEFAULT_REGION} must name a region in {REGIONS}")),
        }

        let mut regions = BTreeMap::new();
        for (index, (code, endpoint)) in list.into_iter().enumerate() {
            if code.is_empty()
                || !code.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(format!("{REGIONS} contains an invalid region code: {code:?}"));
            }
            let candidate = if let Some(endpoint) = endpoint {
                let ip = endpoint.ip();
                if !(1..=65529).contains(&endpoint.port())
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_broadcast()
                {
                    return Err(format!(
                        "{REGIONS} contains an invalid QoS base endpoint for {code}"
                    ));
                }
                Some(Candidate {
                    xuid: Xuid(PROBE_XUID_MARKER | (index as u64 + 1)),
                    endpoint,
                    generation: random_id().map_err(|err| err.to_string())?,
                })
            } else {
                None
            };
            regions.insert(code, candidate);
        }
        Ok(Self { regions, default })
    }

    pub(super) fn candidates(&self) -> impl Iterator<Item = &Candidate> {
        self.regions.values().filter_map(Option::as_ref)
    }

    pub(super) fn accepts(&self, sample: &Sample) -> bool {
        self.candidates().any(|candidate| {
            candidate.xuid == sample.target && candidate.generation == sample.generation
        })
    }

    pub(super) fn pick(&self, members: &[Entrant<'_>]) -> Option<RegionChoice<'_>> {
        let default = self.default.as_deref()?;
        let mut best = None;
        let mut fallback = RegionChoice {
            region: default,
            rtt_ms: BTreeMap::new(),
        };
        for (region, candidate) in &self.regions {
            let region = region.as_str();
            let Some(probe) = candidate else { continue };
            let rtt_ms: BTreeMap<_, _> = members
                .iter()
                .filter_map(|member| {
                    member
                        .samples
                        .get(&probe.xuid)
                        .copied()
                        .flatten()
                        .map(|rtt| (member.xuid, rtt))
                })
                .collect();
            let choice = RegionChoice { region, rtt_ms };
            if choice.rtt_ms.len() == members.len()
                && let Some(&worst) = choice.rtt_ms.values().max()
            {
                let score = (worst, region != default, region);
                if best.as_ref().is_none_or(|(previous, _)| score < *previous) {
                    best = Some((score, choice));
                }
            } else if region == default {
                fallback = choice;
            }
        }
        Some(best.map_or(fallback, |(_, choice)| choice))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(values: &[(&str, &str)]) -> Result<DedicatedConfig, String> {
        DedicatedConfig::from_lookup(|key| {
            values
                .iter()
                .find(|&&(name, _)| name == key)
                .map(|&(_, value)| value.to_owned())
        })
    }

    #[test]
    fn both_switches_must_enable_the_hopper() {
        assert!(config(&[]).unwrap().playlist(1).is_none());
        assert!(config(&[(ENABLED, "1")]).unwrap().playlist(1).is_none());
        let playlists = r#"{"1":{"map":"lockout"},"3":{"map":"zanzibar","variant":"Team Slayer"}}"#;
        assert!(config(&[(HOPPERS_ENABLED, playlists)]).unwrap().playlist(1).is_none());
        assert!(config(&[(ENABLED, "true"), (HOPPERS_ENABLED, playlists)]).unwrap().playlist(1).is_none());
        let enabled = config(&[(ENABLED, "1"), (HOPPERS_ENABLED, playlists)]).unwrap();
        assert!(enabled.playlist(1).is_some());
        assert!(enabled.playlist(3).is_some());
        assert!(enabled.playlist(2).is_none());
        let playlist = enabled.playlist(3).unwrap();
        assert_eq!(playlist.map, "zanzibar");
        assert_eq!(playlist.variant.as_deref(), Some("Team Slayer"));
    }

    fn configured_regions(default: &str) -> RegionTable {
        config(&[
            (ENABLED, "1"),
            (HOPPERS_ENABLED, r#"{"1":{"map":"lockout"}}"#),
            (REGIONS, r#"{"ams":"192.0.2.1:31000","lhr":"192.0.2.2:31000","iad":null}"#),
            (DEFAULT_REGION, default),
        ])
        .unwrap()
        .regions
    }

    #[test]
    fn chooses_the_region_with_the_lowest_worst_member_rtt() {
        let regions = configured_regions("iad");
        let mut probes = regions.candidates();
        let ams = probes.next().unwrap().xuid;
        let lhr = probes.next().unwrap().xuid;
        let a = BTreeMap::from([(ams, Some(10)), (lhr, Some(55))]);
        let b = BTreeMap::from([(ams, Some(85)), (lhr, Some(60))]);
        let members = [
            Entrant { xuid: Xuid(1), level: 1, has_map_pack: false, since: 0, samples: &a },
            Entrant { xuid: Xuid(2), level: 1, has_map_pack: false, since: 0, samples: &b },
        ];

        let selected = regions.pick(&members).unwrap();
        assert_eq!(selected.region, "lhr");
        assert_eq!(selected.rtt_ms, BTreeMap::from([(Xuid(1), 55), (Xuid(2), 60)]));
    }

    #[test]
    fn uses_the_default_when_no_region_has_rtts_for_every_member() {
        let regions = configured_regions("lhr");
        let mut probes = regions.candidates();
        let ams = probes.next().unwrap().xuid;
        let lhr = probes.next().unwrap().xuid;
        let a = BTreeMap::from([(ams, Some(30)), (lhr, Some(60))]);
        let b = BTreeMap::from([(lhr, None)]);
        let members = [
            Entrant { xuid: Xuid(1), level: 1, has_map_pack: false, since: 0, samples: &a },
            Entrant { xuid: Xuid(2), level: 1, has_map_pack: false, since: 0, samples: &b },
        ];
        let selected = regions.pick(&members).unwrap();
        assert_eq!(selected.region, "lhr");
        assert_eq!(selected.rtt_ms, BTreeMap::from([(Xuid(1), 60)]));

        let absent = BTreeMap::new();
        let a = Entrant { samples: &absent, ..members[0] };
        let b = Entrant { samples: &absent, ..members[1] };
        let selected = regions.pick(&[a, b]).unwrap();
        assert_eq!(selected.region, "lhr");
        assert!(selected.rtt_ms.is_empty());

        let no_probe_default = configured_regions("iad");
        let selected = no_probe_default.pick(&[a, b]).unwrap();
        assert_eq!(selected.region, "iad");
        assert!(selected.rtt_ms.is_empty());
    }

    #[test]
    fn ties_prefer_the_default_if_it_has_complete_measurements() {
        let regions = configured_regions("lhr");
        let samples = regions.candidates().map(|probe| (probe.xuid, Some(15))).collect();
        let members = [Entrant { xuid: Xuid(1), level: 1, has_map_pack: false, since: 0, samples: &samples }];
        assert_eq!(regions.pick(&members).unwrap().region, "lhr");
    }

    #[test]
    fn only_the_current_probe_generation_is_accepted() {
        let regions = configured_regions("iad");
        let probe = regions.candidates().next().unwrap();
        let mut sample = Sample {
            target: probe.xuid,
            generation: probe.generation.clone(),
            rtt_ms: Some(50),
        };
        assert!(regions.accepts(&sample));
        let replacement = if sample.generation.ends_with('0') { "1" } else { "0" };
        sample.generation.replace_range(15.., replacement);
        assert!(!regions.accepts(&sample));
    }

    #[test]
    fn defaults_must_come_from_the_configured_list() {
        assert!(config(&[
            (ENABLED, "1"),
            (HOPPERS_ENABLED, r#"{"1":{"map":"lockout"}}"#),
            (REGIONS, r#"{"iad":null}"#),
            (DEFAULT_REGION, "lhr"),
        ])
        .is_err());
    }
}
