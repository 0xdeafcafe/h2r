//! Moderation follows the normalized image hash across slots and galleries.

use live_proto::screenshots::{CaptureProof, SCREENSHOT_REPORTED, SCREENSHOT_REVIEWED};

use super::*;

pub(in crate::content) const STREAM: &str = "screenshot-moderation";
const STRIKES: usize = 3;
const PAGE_SIZE: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Pending,
    Approved,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Reason {
    ClassifierFlag,
    ClassifierUncertain,
    ClassifierUnavailable,
    GeometryMismatch,
    GeometryUnavailable,
    PrivateCapture,
    PeerCapture,
    PlayerReport,
    Legacy,
}

impl Reason {
    fn label(self) -> &'static str {
        match self {
            Self::ClassifierFlag => "The image classifier flagged this capture.",
            Self::ClassifierUncertain => "The image classifier could not clear this capture.",
            Self::ClassifierUnavailable => "The image classifier is unavailable.",
            Self::GeometryMismatch => "The image does not match the mapped view.",
            Self::GeometryUnavailable => "Live has insufficient map geometry for this view.",
            Self::PrivateCapture => {
                "This capture comes from a private game on the uploader’s device."
            }
            Self::PeerCapture => "This capture comes from a player-hosted game.",
            Self::PlayerReport => "A player reported this capture.",
            Self::Legacy => "This image was uploaded before capture verification.",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Submission {
    pub(super) uploader: Xuid,
    pub(super) status: Status,
    pub(super) reason: Option<Reason>,
    pub(super) nsfw: Option<f32>,
    pub(super) proof: Option<CaptureProof>,
}

impl Submission {
    pub(super) fn legacy(uploader: Xuid) -> Self {
        Self {
            uploader,
            status: Status::Pending,
            reason: Some(Reason::Legacy),
            nsfw: None,
            proof: None,
        }
    }
}

#[derive(Debug)]
struct Record {
    submission: Submission,
    position: u64,
    reported_by: BTreeSet<Xuid>,
}

#[derive(Default, Debug)]
pub(super) struct Reviews {
    records: HashMap<Sha256, Record>,
    pending: BTreeMap<u64, Sha256>,
    strikes: HashMap<Xuid, BTreeSet<Sha256>>,
    pub(super) version: u64,
}

impl Reviews {
    pub(super) fn visible(&self, hash: &Sha256) -> bool {
        self.get(hash).is_none_or(|s| s.status == Status::Approved)
    }

    pub(super) fn get(&self, hash: &Sha256) -> Option<&Submission> {
        self.records.get(hash).map(|r| &r.submission)
    }

    pub(super) fn submit(
        &mut self,
        hash: Sha256,
        position: u64,
        submission: Submission,
    ) -> Option<Xuid> {
        if self.records.contains_key(&hash) {
            return None;
        }
        if submission.status == Status::Pending {
            self.pending.insert(position, hash);
        }
        let flagged = submission.reason == Some(Reason::ClassifierFlag);
        let uploader = submission.uploader;
        self.records.insert(
            hash,
            Record {
                submission,
                position,
                reported_by: BTreeSet::new(),
            },
        );
        if flagged {
            self.strike(uploader, hash)
        } else {
            None
        }
    }

    fn strike(&mut self, uploader: Xuid, hash: Sha256) -> Option<Xuid> {
        let strikes = self.strikes.entry(uploader).or_default();
        (strikes.insert(hash) && strikes.len() >= STRIKES).then_some(uploader)
    }

    fn report(&mut self, event: Reported, position: u64) {
        let Some(record) = self.records.get_mut(&event.sha256) else {
            return;
        };
        if record.submission.status == Status::Removed || !record.reported_by.insert(event.by) {
            return;
        }
        if record.submission.status == Status::Approved {
            record.submission.status = Status::Pending;
            record.submission.reason = Some(Reason::PlayerReport);
            record.position = position;
            self.pending.insert(position, event.sha256);
        }
    }

    fn review(&mut self, event: Reviewed) -> Option<Xuid> {
        let record = self.records.get_mut(&event.sha256)?;
        if record.submission.status != Status::Pending {
            return None;
        }
        self.pending.remove(&record.position);
        let uploader = record.submission.uploader;
        record.submission.status = event.action.status();
        match event.action {
            Action::Approve => {
                if let Some(strikes) = self.strikes.get_mut(&uploader) {
                    strikes.remove(&event.sha256);
                }
                None
            }
            Action::Remove => self.strike(uploader, event.sha256),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Reported {
    sha256: Sha256,
    by: Xuid,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Approve,
    Remove,
}

impl Action {
    fn status(self) -> Status {
        match self {
            Self::Approve => Status::Approved,
            Self::Remove => Status::Removed,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Reviewed {
    sha256: Sha256,
    by: Option<Xuid>,
    action: Action,
}

impl CatalogState {
    pub(in crate::content) fn advance_screenshot_review(&mut self, event: &RecordedEvent) {
        if event.body.stream != STREAM {
            return;
        }
        let reviews = &mut self.screenshots.reviews;
        reviews.version = event.version + 1;
        match event.body.kind.as_str() {
            SCREENSHOT_REPORTED => {
                if let Ok(report) = serde_json::from_value(event.body.data.clone()) {
                    reviews.report(report, event.position);
                }
            }
            SCREENSHOT_REVIEWED => {
                if let Ok(review) = serde_json::from_value(event.body.data.clone())
                    && let Some(owner) = reviews.review(review)
                {
                    self.uploads_off.insert(owner);
                }
            }
            _ => {}
        }
    }
}

pub(super) async fn append(live: &Live, events: Vec<NewEvent>, version: u64) -> ApiResult<u64> {
    let recorded = live
        .store
        .append(events, &[(STREAM.into(), version)])
        .await?;
    let position = recorded.last().expect("screenshot event").position;
    live.wait_applied(position).await?;
    Ok(position)
}

#[derive(Deserialize)]
pub(crate) struct ReviewQuery {
    cursor: Option<u64>,
}

pub(crate) async fn list(
    State(live): Shared,
    headers: HeaderMap,
    Query(query): Query<ReviewQuery>,
) -> ApiResult<Response> {
    live.moderator(bearer(&headers))?;
    let state = live.catalog.read();
    let reviews = &state.screenshots.reviews;
    let mut pending = reviews.pending.range((
        std::ops::Bound::Excluded(query.cursor.unwrap_or(0)),
        std::ops::Bound::Unbounded,
    ));
    let mut items = Vec::new();
    let mut cursor = 0;
    for (position, hash) in pending.by_ref().take(PAGE_SIZE) {
        let s = reviews.get(hash).expect("indexed review");
        items.push(json!({
            "sha256": hash,
            "revision": position.to_string(),
            "uploader": s.uploader,
            "reason": s.reason.map(Reason::label),
            "nsfw": s.nsfw,
            "proof": s.proof,
        }));
        cursor = *position;
    }
    let next = pending.next().map(|_| cursor.to_string());
    Ok(crate::http::json(
        StatusCode::OK,
        &json!({"items":items, "next":next}),
    ))
}

pub(crate) async fn preview(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(hash): UrlPath<Sha256>,
) -> ApiResult<Response> {
    live.moderator(bearer(&headers))?;
    {
        let state = live.catalog.read();
        if state
            .screenshots
            .reviews
            .get(&hash)
            .is_none_or(|s| s.status != Status::Pending)
            || !state.blob_standing(&hash).is_ok()
        {
            return Err(not_found("no pending screenshot"));
        }
    }
    let image = live
        .content_store()?
        .get(&blob_key(&hash))
        .await
        .map_err(|_| unavailable("the content store cannot read this image"))?
        .ok_or_else(|| not_found("no such screenshot image"))?;
    if maps::image_type(&image) != Some("image/webp") {
        return Err(not_found("no normalized screenshot image"));
    }
    Ok((
        [
            (header::CONTENT_TYPE, "image/webp"),
            (header::CACHE_CONTROL, crate::caching::PRIVATE),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        image,
    )
        .into_response())
}

pub(crate) async fn report(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath(hash): UrlPath<Sha256>,
) -> ApiResult<Response> {
    let author = live.authenticate(bearer(&headers), lxm::CONTENT_WRITE)?;
    let Author::Account { xuid: by, .. } = author else {
        return Err(forbidden("reports come from accounts"));
    };
    live.rate_limit(&author, lxm::CONTENT_WRITE)?;
    let _guard = live.content_writes.lock().await;
    live.wait_applied(live.store.head().await?).await?;
    let version = {
        let state = live.catalog.read();
        let reviews = &state.screenshots.reviews;
        let record = reviews
            .records
            .get(&hash)
            .ok_or_else(|| not_found("no such screenshot"))?;
        if record.reported_by.contains(&by) {
            return Ok(ok(live.position()));
        }
        if record.submission.status != Status::Approved || !state.servable(&hash) {
            return Err(not_found("no visible screenshot"));
        }
        reviews.version
    };
    let event = live.server_event(
        STREAM.into(),
        SCREENSHOT_REPORTED,
        serde_json::to_value(Reported { sha256: hash, by }).expect("serializes"),
    );
    Ok(ok(append(&live, vec![event], version).await?))
}

#[derive(Deserialize)]
pub(crate) struct ReviewActionQuery {
    revision: u64,
}

pub(crate) async fn moderate(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((hash, action)): UrlPath<(Sha256, String)>,
    Query(query): Query<ReviewActionQuery>,
) -> ApiResult<Response> {
    let by = live.moderator(bearer(&headers))?;
    let action = match action.as_str() {
        "approve" => Action::Approve,
        "remove" => Action::Remove,
        _ => return Err(invalid("the screenshot action is approve or remove")),
    };
    let _guard = live.content_writes.lock().await;
    live.wait_applied(live.store.head().await?).await?;
    let version = {
        let state = live.catalog.read();
        let reviews = &state.screenshots.reviews;
        let record = reviews
            .records
            .get(&hash)
            .ok_or_else(|| not_found("no such screenshot"))?;
        if record.position != query.revision {
            return Err(conflict(
                "the screenshot review has changed, refresh the queue",
            ));
        }
        if record.submission.status == action.status() {
            None
        } else {
            if record.submission.status != Status::Pending {
                return Err(conflict("this screenshot is no longer pending review"));
            }
            if matches!(action, Action::Approve) && !state.blob_standing(&hash).is_ok() {
                return Err(gone("the screenshot content was taken down"));
            }
            Some(reviews.version)
        }
    };
    let position = if let Some(version) = version {
        let event = live.server_event(
            STREAM.into(),
            SCREENSHOT_REVIEWED,
            serde_json::to_value(Reviewed {
                sha256: hash,
                by,
                action,
            })
            .expect("serializes"),
        );
        append(&live, vec![event], version).await?
    } else {
        live.position()
    };
    if matches!(action, Action::Remove) {
        // The durable tombstone prevents a failed or retried blob delete
        // from making a removed hash public again.
        if let Ok(store) = live.content_store()
            && let Err(err) = store.delete(&blob_key(&hash)).await
        {
            eprintln!("live: could not delete removed screenshot {hash}: {err}");
        }
    }
    Ok(ok(position))
}
