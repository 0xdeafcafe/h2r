use hmac::{Hmac, Mac};
use live_proto::game;
use live_proto::screenshots::{CameraPose, CapturePermit, CaptureProof, PROOF_HEADER};
use live_proto::sign::{hex, random_id};

use super::*;

const PERMIT_SECONDS: i64 = 15 * 60;
const MAX_PROOF_BYTES: usize = 2048;

pub(super) enum Source {
    Private,
    PeerGame,
    DedicatedGame,
}

pub(super) fn read(headers: &HeaderMap) -> ApiResult<CaptureProof> {
    let entries = headers.get_all(PROOF_HEADER);
    let mut entries = entries.iter();
    let proof = entries
        .next()
        .ok_or_else(|| invalid("a screenshot needs in-game capture proof"))?;
    if entries.next().is_some() {
        return Err(invalid("send exactly one capture proof"));
    }
    if proof.len() > MAX_PROOF_BYTES {
        return Err(invalid("capture proof is too large"));
    }
    let proof: CaptureProof =
        serde_json::from_slice(proof.as_bytes()).map_err(|_| invalid("invalid capture proof"))?;
    if !game::valid_id(&proof.game_id)
        || !game::valid_id(&proof.map)
        || proof.tick == 0
        || !valid_camera(&proof.camera)
    {
        return Err(invalid("invalid capture game, map, tick or camera"));
    }
    Ok(proof)
}

fn valid_camera(camera: &CameraPose) -> bool {
    let dot = |a: [f32; 3], b: [f32; 3]| a.into_iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
    camera
        .position
        .iter()
        .all(|p| p.is_finite() && p.abs() <= 1_000_000.0)
        && (0.95..=1.05).contains(&dot(camera.forward, camera.forward))
        && (0.95..=1.05).contains(&dot(camera.up, camera.up))
        && dot(camera.forward, camera.up).abs() <= 0.05
}

fn permit_mac(live: &Live, owner: Xuid, map: MapId, id: &str, expires: i64) -> Hmac<sha2::Sha256> {
    let key = live.identity.derive("live/gallery-capture-permit/v1");
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&key).expect("32-byte HMAC key");
    mac.update(format!("{owner}:{map}:{id}:{expires}").as_bytes());
    mac
}

pub(crate) async fn create(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(id): UrlPath<MapId>,
) -> ApiResult<Response> {
    let (_, owner) = live.uploader(bearer(&headers))?;
    live.catalog
        .read()
        .authorize_gallery(Target::Map, &id.to_string(), owner)?;
    let game_id = format!("capture-{}", random_id());
    let expires = now() + PERMIT_SECONDS;
    let signature = permit_mac(&live, owner, id, &game_id, expires)
        .finalize()
        .into_bytes();
    let permit = CapturePermit {
        game_id,
        token: format!("{expires}.{}", hex(&signature)),
    };
    Ok(crate::http::json(StatusCode::CREATED, &permit))
}

pub(super) fn verify(live: &Live, owner: Xuid, proof: &CaptureProof) -> ApiResult<Source> {
    if let Some(token) = &proof.permit {
        let (expires, signature) = token
            .split_once('.')
            .and_then(|(expires, mac)| {
                Some((expires.parse::<i64>().ok()?, mac.parse::<Sha256>().ok()?))
            })
            .ok_or_else(|| forbidden("invalid private capture permit"))?;
        let clock = now();
        if !(clock..=clock + PERMIT_SECONDS).contains(&expires)
            || !proof.game_id.starts_with("capture-")
            || permit_mac(live, owner, proof.map_id, &proof.game_id, expires)
                .verify_slice(&signature.0)
                .is_err()
        {
            return Err(forbidden(
                "private capture permit expired or belongs to another game, map or account",
            ));
        }
        return Ok(Source::Private);
    }

    let model = live.model.read();
    let game = model
        .games
        .get(&proof.game_id)
        .ok_or_else(|| forbidden("capture game is not recorded on this server"))?;
    if !game.xuids.contains(&owner) {
        return Err(forbidden("only a participant can upload a game capture"));
    }
    if game.started.game.map_id != Some(proof.map_id)
        || !game.started.game.map.eq_ignore_ascii_case(&proof.map)
    {
        return Err(forbidden("capture map does not match the recorded game"));
    }
    let first = game.started.game.tick.unwrap_or(0);
    let last = game
        .ended
        .as_ref()
        .map(|(end, _)| end.tick)
        .or(game.last_tick)
        .or(game.started.game.tick);
    if last.is_none_or(|last| !(first..=last).contains(&proof.tick)) {
        return Err(forbidden("capture tick is outside the recorded game"));
    }
    Ok(if game.started.dedicated.host_lease.is_some() {
        Source::DedicatedGame
    } else {
        Source::PeerGame
    })
}
