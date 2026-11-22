//! Shared by the Worker and its native helper tests.

use std::collections::BTreeMap;

use live_proto::host_pack::{AddMatch, class_limits, valid_pack};
use live_proto::hosts::{HostClass, HostLease, HostState, ProviderKind, Transport};
use live_proto::{ErrorCode, Xuid};
use live_server::{ApiError, hosts::HostBoot};

pub(crate) use live_proto::host_pack::{CONTROL_PORT, TUNNEL_PORT};

pub(crate) fn tunnel_lease(path: &str) -> Option<&str> {
    let id = path.strip_prefix("/v1/hosts/")?.strip_suffix("/tunnel")?;
    valid_id(id).then_some(id)
}

pub(crate) fn valid_id(id: &str) -> bool {
    let Some(token) = id.strip_prefix("hl-") else {
        return false;
    };
    token.len() == 22
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub(crate) fn binding(class: &HostClass) -> &'static str {
    match class {
        HostClass::Small => "HOST_ROOM",
        HostClass::Large => "HOST_ROOM_LARGE",
    }
}

pub(crate) fn provider_ref(lease: &HostLease) -> String {
    format!("{}/{}", binding(&lease.class), lease.id)
}

pub(crate) fn parse_ref(reference: &str) -> Option<(&str, &str)> {
    let (namespace, id) = reference.split_once('/')?;
    (matches!(namespace, "HOST_ROOM" | "HOST_ROOM_LARGE") && valid_id(id))
        .then_some((namespace, id))
}

pub(crate) fn admit(lease: &HostLease, state: &HostState, xuid: Xuid) -> Result<(), ApiError> {
    if !lease.roster.contains(&xuid) {
        return Err(ApiError::new(
            ErrorCode::Forbidden,
            "the account has no seat on this host",
        ));
    }
    if lease.provider != ProviderKind::Cloudflare
        || lease.transport != Transport::Tunnel
        || !matches!(state, HostState::Joinable | HostState::Playing)
    {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "the host tunnel is not available",
        ));
    }
    Ok(())
}

/// One lease's match for its pack. This travels on private room-to-room
/// and room-to-container requests and is never written to storage.
pub(crate) fn add_match(lease: &HostLease, boot: &HostBoot) -> serde_json::Result<AddMatch> {
    Ok(AddMatch {
        lease: lease.id.clone(),
        lease_json: serde_json::to_string(lease)?,
        live: boot.live_url.clone(),
        token: boot.token.clone(),
        data_url: boot.data_url.clone(),
        data_sha: boot.data_sha256.clone(),
        archive: boot.archive,
        region: lease.region.clone(),
    })
}

/// Cloudflare Durable Object location hints. Other providers also put
/// region names in leases, including three-letter airport codes.
pub(crate) fn location_hint(region: Option<&str>) -> Option<&'static str> {
    const HINTS: [&str; 9] = ["wnam", "enam", "sam", "weur", "eeur", "apac", "oc", "afr", "me"];
    let region = region?;
    HINTS.into_iter().find(|hint| *hint == region)
}

/// The room that routes a class's leases in one location to its packs.
/// Packs only share matches placed in the same location.
pub(crate) fn pool_name(region: Option<&str>) -> String {
    format!("pool-{}", location_hint(region).unwrap_or("any"))
}

pub(crate) fn pool_hint(pool: &str) -> Option<&'static str> {
    location_hint(pool.strip_prefix("pool-"))
}

pub(crate) fn valid_pool(name: &str) -> bool {
    name == "pool-any" || pool_hint(name).is_some()
}

/// The room that owns one pack's container.
pub(crate) fn pack_room(pool: &str, pack: &str) -> String {
    format!("{pool}/{pack}")
}

pub(crate) fn parse_pack_room(name: &str) -> Option<(&str, &str)> {
    let (pool, pack) = name.split_once('/')?;
    (valid_pool(pool) && valid_pack(pack)).then_some((pool, pack))
}

/// What a room named `name` does.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// The hub's handle for one lease.
    Lease,
    /// Routes leases to packs.
    Pool,
    /// Owns one container of several matches.
    Pack,
}

pub(crate) fn role(name: &str) -> Option<Role> {
    if valid_id(name) {
        Some(Role::Lease)
    } else if valid_pool(name) {
        Some(Role::Pool)
    } else if parse_pack_room(name).is_some() {
        Some(Role::Pack)
    } else {
        None
    }
}

/// Operator cap, bounded by the class's slot and port allocation.
pub(crate) fn pack_matches(class: &HostClass, setting: Option<&str>) -> usize {
    class_limits(class, setting.and_then(|v| v.trim().parse().ok())).max_matches
}

/// The supervisor uses the same admission envelope on both homes. A capped
/// Cloudflare application provisions at least 3 GiB per vCPU in Wrangler.
pub(crate) fn pack_env(class: &HostClass, max_matches: usize) -> BTreeMap<&'static str, String> {
    let limits = class_limits(class, Some(max_matches));
    BTreeMap::from([
        ("H2_PACK", "1".to_owned()),
        ("H2_PACK_MAX_MATCHES", limits.max_matches.to_string()),
        ("H2_PACK_VCPUS", limits.vcpus.to_string()),
        ("H2_PACK_MEMORY_MIB", limits.memory_mib.to_string()),
        ("H2_PACK_CONTROL", format!("0.0.0.0:{CONTROL_PORT}")),
        ("H2_TUNNEL_LISTEN", format!("0.0.0.0:{TUNNEL_PORT}")),
    ])
}
