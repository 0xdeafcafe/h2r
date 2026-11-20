use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use live_proto::film::{MAX_UPLOAD_BYTES, PART_BYTES, SINGLE_UPLOAD_BYTES};
use sha2::{Digest, Sha256};

pub const MAX_UPLOAD: u64 = MAX_UPLOAD_BYTES;
/// The supervisor waits this long for the whole upload across all requests.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// A completed host film, sent after the match using its telemetry.ingest token.
pub fn upload(live_url: &str, token: &str, game_id: &str, path: impl AsRef<Path>) -> Result<()> {
    let mut url = url::Url::parse(live_url)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none()
            && live_proto::bearer_allowed(url.as_str()),
        "film upload requires HTTPS or a loopback Live URL"
    );
    ensure!(
        url.query().is_none() && url.fragment().is_none(),
        "invalid Live base URL"
    );
    ensure!(
        !game_id.is_empty() && game_id != "." && game_id != "..",
        "missing or invalid game ID"
    );
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid Live URL"))?
        .pop_if_empty()
        .extend(["v1", "games", game_id, "film"]);
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    ensure!(size <= MAX_UPLOAD, "film exceeds 2 GiB");
    ensure!(size > 0, "empty host film");
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let deadline = Instant::now() + UPLOAD_TIMEOUT;
    let request = |method: &str, url: &url::Url, len: u64| -> Result<ureq::Request> {
        let left = deadline
            .checked_duration_since(Instant::now())
            .context("film upload timed out")?;
        Ok(agent
            .request(method, url.as_str())
            .timeout(left)
            .set("Authorization", &format!("Bearer {token}"))
            .set("X-H2-Film-Continue", "1")
            .set("Content-Type", "application/octet-stream")
            .set("Content-Length", &len.to_string()))
    };
    let mut next = url.clone();
    next.query_pairs_mut().append_pair("continue", "1");
    let acknowledge = |mut status: u16| -> Result<()> {
        while status == 202 {
            status = send_status(|| Ok(request("POST", &next, 0)?.send_bytes(&[])?))?;
        }
        ensure!(status == 204, "unexpected film upload status {status}");
        Ok(())
    };
    if size <= SINGLE_UPLOAD_BYTES {
        let status = send_status(|| {
            file.rewind()?;
            Ok(request("POST", &url, size)?.send((&mut file).take(size))?)
        })?;
        return acknowledge(status);
    }

    let mut hash = Sha256::new();
    let mut buf = vec![0; PART_BYTES as usize];
    for part in 0..size.div_ceil(PART_BYTES) {
        let len = (size - part * PART_BYTES).min(PART_BYTES);
        let bytes = &mut buf[..len as usize];
        file.read_exact(bytes)?;
        hash.update(&*bytes);
        let mut part_url = url.clone();
        part_url
            .query_pairs_mut()
            .append_pair("part", &part.to_string());
        let status = send_status(|| Ok(request("PUT", &part_url, len)?.send_bytes(bytes)?))
            .with_context(|| format!("upload film part {part}"))?;
        acknowledge(status).with_context(|| format!("process film part {part}"))?;
    }
    url.query_pairs_mut()
        .append_pair("complete", &size.to_string())
        .append_pair("sha256", &format!("{:x}", hash.finalize()));
    let status = send_status(|| Ok(request("POST", &url, 0)?.send_bytes(&[])?))?;
    acknowledge(status)
}

/// A response can be lost after Live has stored the bytes or advanced its
/// cursor. The upload protocol makes repeating that exact request safe.
fn send_status(mut send: impl FnMut() -> Result<ureq::Response>) -> Result<u16> {
    let response = match send() {
        Err(error)
            if matches!(
                error.downcast_ref::<ureq::Error>(),
                Some(ureq::Error::Transport(_) | ureq::Error::Status(500..=599, _))
            ) => send()?,
        result => result?,
    };
    Ok(response.status())
}
