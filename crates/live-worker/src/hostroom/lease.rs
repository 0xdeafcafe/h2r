//! Shared by the Worker and its native helper tests.

use live_proto::hosts::{HostClass, HostLease, HostState, ProviderKind, Transport};
use live_proto::{ErrorCode, Xuid};
use live_server::{ApiError, hosts::HostBoot};
use serde::{Deserialize, Serialize};

pub(crate) const TUNNEL_PORT: u16 = 31100;

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

/// The complete environment handed to h2. This travels on the private
/// start request and is never written to room storage.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) struct HostEnv {
    pub(crate) h2_lease: String,
    h2_host_lease_json: String,
    h2_live: String,
    h2_token: String,
    h2_data_url: String,
    h2_data_sha: String,
    #[serde(default)]
    h2_host_archive: live_proto::hosts::HostArchive,
    #[serde(skip_serializing_if = "Option::is_none")]
    h2_region: Option<String>,
    h2_agent: String,
    h2_tunnel_listen: String,
}

impl HostEnv {
    pub(crate) fn new(lease: &HostLease, boot: &HostBoot) -> serde_json::Result<Self> {
        Ok(Self {
            h2_lease: lease.id.clone(),
            h2_host_lease_json: serde_json::to_string(lease)?,
            h2_live: boot.live_url.clone(),
            h2_token: boot.token.clone(),
            h2_data_url: boot.data_url.clone(),
            h2_data_sha: boot.data_sha256.clone(),
            h2_host_archive: boot.archive,
            h2_region: lease.region.clone(),
            // Port 31100 is reserved for the WebSocket tunnel. The diagnostic
            // agent is private even inside the container network.
            h2_agent: "tcp://127.0.0.1:14720".into(),
            h2_tunnel_listen: format!("0.0.0.0:{TUNNEL_PORT}"),
        })
    }
}
