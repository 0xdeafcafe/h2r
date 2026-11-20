use live_proto::{
    Xuid,
    matchmaking::{Connectivity, Nat},
};
use std::collections::BTreeMap;

pub(super) const MIN_UPLOAD_KBPS: u32 = 1_000;

/// A timeout is present with no RTT. An absent entry has never been probed
/// in this ticket generation.
pub(super) type Samples = BTreeMap<Xuid, Option<u16>>;

#[derive(Clone, Copy)]
pub(super) struct Entrant<'a> {
    pub xuid: Xuid,
    pub level: u8,
    pub since: i64,
    pub samples: &'a Samples,
}

#[derive(Clone, Copy)]
pub(super) struct Host {
    pub xuid: Xuid,
    pub dedicated: bool,
    pub connectivity: Connectivity,
}

impl Host {
    fn eligible(&self) -> bool {
        self.connectivity.nat != Some(Nat::Strict)
            && self.connectivity.upload_kbps.is_none_or(|kbps| kbps >= MIN_UPLOAD_KBPS)
    }

    fn rtt(self, member: &Entrant<'_>) -> Option<u16> {
        if self.xuid == member.xuid {
            Some(0)
        } else {
            member.samples.get(&self.xuid).copied().flatten()
        }
    }
}

pub(super) struct Selected {
    pub host: Host,
    pub rtt_ms: BTreeMap<Xuid, u16>,
}

/// Rank only hosts with a successful inbound route from every member.
pub(super) fn select_host(members: &[Entrant<'_>], hosts: &[Host]) -> Option<Selected> {
    if members.is_empty() {
        return None;
    }
    hosts
        .iter()
        .copied()
        .filter(|host| host.eligible())
        .filter(|host| host.dedicated || members.iter().any(|p| p.xuid == host.xuid))
        .filter_map(|host| {
            let rtt_ms: BTreeMap<_, _> = members
                .iter()
                .map(|member| Some((member.xuid, host.rtt(member)?)))
                .collect::<Option<_>>()?;
            // The local player has no network round trip. Keep zero in
            // the table but leave it out of the median for scoring.
            let mut routes: Vec<_> = rtt_ms
                .iter()
                .filter_map(|(&member, &rtt)| (member != host.xuid).then_some(rtt))
                .collect();
            routes.sort_unstable();
            let worst = routes.last().copied().unwrap_or(0);
            let median_twice = if routes.is_empty() {
                0
            } else {
                u32::from(routes[(routes.len() - 1) / 2]) + u32::from(routes[routes.len() / 2])
            };
            // Dedicated must improve the worst route to displace a peer.
            Some(((worst, host.dedicated, median_twice, host.xuid), Selected { host, rtt_ms }))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, selected)| selected)
}

fn windows(wait_s: i64) -> (u8, u16) {
    let steps = (wait_s.max(0) / 5).min(15);
    let skill = (5 + 3 * steps).min(49) as u8;
    let latency = (90 + 20 * steps).min(350) as u16;
    (skill, latency)
}

/// Find a cohort around the oldest matchable entrant. A single eligible
/// host must have acceptable routes to the entire group. Iterate reachable
/// skill windows per host so a high-latency nearest neighbor cannot block
/// another compatible group.
pub(super) fn group<'a>(
    entrants: &[Entrant<'a>],
    hosts: &[Host],
    size: usize,
    now: i64,
) -> Option<Vec<Entrant<'a>>> {
    if size < 2 || entrants.len() < size {
        return None;
    }
    let hosts: Vec<_> = hosts.iter().copied().filter(Host::eligible).collect();
    let mut oldest = entrants.to_vec();
    oldest.sort_unstable_by_key(|p| (p.since, p.xuid));
    for seed in oldest {
        let (skill, latency) = windows(now.saturating_sub(seed.since));
        let mut best = None;
        for &host in &hosts {
            if host.rtt(&seed).is_none_or(|rtt| rtt > latency) {
                continue;
            }
            let mut reachable: Vec<_> = entrants
                .iter()
                .copied()
                .filter(|p| host.rtt(p).is_some_and(|rtt| rtt <= latency))
                .collect();
            // Seed and peer host are required even when other players
            // are closer in skill. All remaining seats go to the players
            // nearest the seed within the current skill band.
            let required = |p: &Entrant<'_>| p.xuid == seed.xuid || (!host.dedicated && p.xuid == host.xuid);
            reachable.sort_unstable_by_key(|p| (!required(p), seed.level.abs_diff(p.level), p.level, p.xuid));
            for low in seed.level.saturating_sub(skill)..=seed.level {
                let mut group: Vec<_> = reachable
                    .iter()
                    .copied()
                    .filter(|p| (low..=low.saturating_add(skill)).contains(&p.level))
                    .take(size)
                    .collect();
                if group.len() != size || (!host.dedicated && !group.iter().any(|p| p.xuid == host.xuid)) {
                    continue;
                }
                // Wait for every eligible contender to be attempted, so
                // request order does not elect the first one measured.
                if !group.iter().all(|p| {
                    hosts.iter().all(|h| {
                        h.xuid == p.xuid
                            || (!h.dedicated && !group.iter().any(|m| m.xuid == h.xuid))
                            || p.samples.contains_key(&h.xuid)
                    })
                }) {
                    continue;
                }
                group.sort_unstable_by_key(|p| (p.level, p.xuid));
                let spread = group[size - 1].level - group[0].level;
                let distance: u32 = group.iter().map(|p| u32::from(seed.level.abs_diff(p.level))).sum();
                let ids: Vec<_> = group.iter().map(|p| p.xuid).collect();
                let score = (spread, distance, ids);
                if best.as_ref().is_none_or(|(previous, _)| score < *previous) {
                    best = Some((score, group));
                }
            }
        }
        if let Some((_, group)) = best {
            return Some(group);
        }
    }
    None
}

/// Skill is sorted high to low and distributed in a snake so cardinality
/// stays equal and successive pick rounds alternate which team picks first.
pub(super) fn teams(members: &[Entrant<'_>], count: usize) -> BTreeMap<Xuid, u8> {
    if count == 0 {
        return BTreeMap::new();
    }
    let mut members = members.to_vec();
    members.sort_unstable_by_key(|p| (std::cmp::Reverse(p.level), p.xuid));
    members
        .iter()
        .enumerate()
        .map(|(index, p)| {
            let position = index % count;
            let team = if (index / count) % 2 == 0 {
                position
            } else {
                count - 1 - position
            };
            (p.xuid, team as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: u64) -> Host {
        Host { xuid: Xuid(id), dedicated: false, connectivity: Connectivity::default() }
    }

    fn samples(values: &[(u64, Option<u16>)]) -> Samples {
        values.iter().map(|&(id, rtt)| (Xuid(id), rtt)).collect()
    }

    fn entrant(id: u64, level: u8, samples: &Samples) -> Entrant<'_> {
        Entrant { xuid: Xuid(id), level, since: 0, samples }
    }

    #[test]
    fn worst_then_true_median_then_xuid_choose_the_host() {
        let a = samples(&[(2, Some(80)), (3, Some(10)), (4, Some(100))]);
        let b = samples(&[(1, Some(80)), (3, Some(20)), (4, Some(100))]);
        let c = samples(&[(1, Some(50)), (2, Some(30)), (4, Some(100))]);
        let d = samples(&[(1, Some(60)), (2, Some(70)), (3, Some(81))]);
        let members = [entrant(1, 10, &a), entrant(2, 10, &b), entrant(3, 10, &c), entrant(4, 10, &d)];
        let mut hosts = [peer(4), peer(3), peer(2), peer(1)];
        let selected = select_host(&members, &hosts).unwrap();
        assert_eq!(selected.host.xuid, Xuid(1));
        assert_eq!(selected.rtt_ms, BTreeMap::from([
            (Xuid(1), 0), (Xuid(2), 80), (Xuid(3), 50), (Xuid(4), 60),
        ]));
        hosts.reverse();
        assert_eq!(select_host(&members, &hosts).unwrap().host.xuid, Xuid(1));

        let tie_a = samples(&[(2, Some(40))]);
        let tie_b = samples(&[(1, Some(40))]);
        let tied = [entrant(1, 10, &tie_a), entrant(2, 10, &tie_b)];
        assert_eq!(select_host(&tied, &[peer(2), peer(1)]).unwrap().host.xuid, Xuid(1));
    }

    #[test]
    fn an_unmeasured_or_unreachable_route_cannot_win() {
        let a = samples(&[(2, None)]);
        let b = samples(&[(1, Some(50))]);
        let members = [entrant(1, 1, &a), entrant(2, 1, &b)];
        assert_eq!(select_host(&members, &[peer(1), peer(2)]).unwrap().host.xuid, Xuid(1));
        let a = Samples::new();
        let members = [entrant(1, 1, &a), entrant(2, 1, &b)];
        assert!(group(&members, &[peer(1), peer(2)], 2, 0).is_none());
        assert!(select_host(&members, &[peer(2)]).is_none());
    }

    #[test]
    fn all_bad_nat_or_upload_leaves_the_group_waiting() {
        let a = samples(&[(2, Some(10))]);
        let b = samples(&[(1, Some(10))]);
        let members = [entrant(1, 1, &a), entrant(2, 1, &b)];
        let mut strict = peer(1);
        strict.connectivity.nat = Some(Nat::Strict);
        let mut slow = peer(2);
        slow.connectivity.upload_kbps = Some(MIN_UPLOAD_KBPS - 1);
        assert!(select_host(&members, &[strict, slow]).is_none());
        assert!(group(&members, &[strict, slow], 2, 1000).is_none());
    }

    #[test]
    fn dedicated_must_improve_the_worst_round_trip() {
        let mut dedicated = peer(3);
        dedicated.dedicated = true;
        let a = samples(&[(2, Some(45)), (3, Some(24))]);
        let b = samples(&[(1, Some(40)), (3, Some(35))]);
        let members = [entrant(1, 1, &a), entrant(2, 1, &b)];
        let hosts = [peer(1), peer(2), dedicated];
        assert!(select_host(&members, &hosts).unwrap().host.dedicated);

        let b = samples(&[(1, Some(40)), (3, Some(40))]);
        let tied = [entrant(1, 1, &a), entrant(2, 1, &b)];
        assert_eq!(select_host(&tied, &hosts).unwrap().host.xuid, Xuid(1));
    }

    #[test]
    fn waiting_widens_both_skill_and_latency_windows() {
        let a = samples(&[(2, Some(125))]);
        let b = samples(&[(1, Some(120))]);
        let entrants = [entrant(1, 12, &a), entrant(2, 20, &b)];
        let hosts = [peer(1), peer(2)];
        assert!(group(&entrants, &hosts, 2, 0).is_none());
        assert!(group(&entrants, &hosts, 2, 5).is_none());
        assert_eq!(group(&entrants, &hosts, 2, 10).unwrap().len(), 2);
    }

    #[test]
    fn required_peer_host_can_sit_at_the_far_end_of_a_skill_band() {
        let a = samples(&[(4, Some(20))]);
        let b = samples(&[(4, Some(20))]);
        let c = samples(&[(4, Some(20))]);
        let d = Samples::new();
        let entrants = [
            entrant(1, 5, &a),
            entrant(2, 6, &b),
            entrant(3, 7, &c),
            entrant(4, 10, &d),
        ];
        let group = group(&entrants, &[peer(4)], 2, 0).unwrap();
        assert_eq!(group[0].xuid, Xuid(1));
        assert_eq!(group[1].xuid, Xuid(4));
    }

    #[test]
    fn skips_a_close_skill_neighbor_when_another_has_a_better_route() {
        let a = samples(&[(2, Some(200)), (3, Some(40))]);
        let b = samples(&[(1, Some(200)), (3, Some(200))]);
        let c = samples(&[(1, Some(40)), (2, Some(200))]);
        let entrants = [entrant(1, 10, &a), entrant(2, 10, &b), entrant(3, 11, &c)];
        let hosts = [peer(1), peer(2), peer(3)];
        let group = group(&entrants, &hosts, 2, 0).unwrap();
        assert_eq!(group.iter().map(|p| p.xuid).collect::<Vec<_>>(), [Xuid(1), Xuid(3)]);
        assert_eq!(teams(&group, 2), BTreeMap::from([(Xuid(1), 1), (Xuid(3), 0)]));
    }
}
