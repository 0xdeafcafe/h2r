//! Uploader-owned gallery slots. A slot update and any new content record
//! share one append, so replay sees the same gallery as the publishing node.
mod geometry;
pub(crate) mod proof;
pub(super) mod review;
pub(crate) use review::{list, moderate, preview, report};

use super::*;
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageFormat, ImageReader, Limits};
use live_proto::screenshots::CaptureProof;
use std::io::Cursor;

const MAX_INPUT: usize = 5 * 1024 * 1024;
const MAX_SIDE: u32 = 4096;
const MAX_PIXELS: u64 = 8_000_000;
const MAX_DECODED: u64 = 32 * 1024 * 1024;
const DISPLAY_SIDE: u32 = 1280;
const NSFW_REVIEW_SCORE: f32 = 0.80;
const NSFW_CLEAR_SCORE: f32 = 0.20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Upload,
    Capture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Slot {
    kind: Kind,
    slot: u8,
}

impl Slot {
    fn parse(kind: &str, slot: &str) -> ApiResult<Self> {
        let (kind, count) = match kind {
            "upload" => (Kind::Upload, 3),
            "capture" => (Kind::Capture, 5),
            _ => return Err(invalid("image kind must be upload or capture")),
        };
        let slot = slot
            .parse::<u8>()
            .ok()
            .filter(|n| *n < count)
            .ok_or_else(|| invalid(format!("image slot must be between 0 and {}", count - 1)))?;
        Ok(Self { kind, slot })
    }
}

#[derive(Serialize, Deserialize)]
struct ScreenshotSet {
    target: Target,
    id: String,
    #[serde(flatten)]
    slot: Slot,
    sha256: Sha256,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    submission: Option<review::Submission>,
}

#[derive(Debug, Default)]
pub(super) struct Galleries {
    slots: BTreeMap<(Target, String), BTreeMap<Slot, Sha256>>,
    reviews: review::Reviews,
}

impl CatalogState {
    pub(super) fn apply_screenshot(&mut self, event: &RecordedEvent) {
        let Ok(shot) = serde_json::from_value::<ScreenshotSet>(event.body.data.clone()) else {
            return;
        };
        let submission = shot.submission.or_else(|| {
            if event.body.stream == review::STREAM {
                return None;
            }
            self.blobs
                .get(&shot.sha256)
                .map(|blob| review::Submission::legacy(blob.uploader))
        });
        if let Some(submission) = submission
            && let Some(owner) =
                self.screenshots
                    .reviews
                    .submit(shot.sha256, event.position, submission)
        {
            self.uploads_off.insert(owner);
        }
        self.screenshots
            .slots
            .entry((shot.target, shot.id))
            .or_default()
            .insert(shot.slot, shot.sha256);
    }

    fn screenshot_hashes(&self, target: Target, id: &str) -> impl Iterator<Item = Sha256> + '_ {
        self.screenshots
            .slots
            .get(&(target, id.to_owned()))
            .into_iter()
            .flat_map(|slots| slots.values().copied())
    }

    pub(super) fn screenshot_visible(&self, hash: &Sha256) -> bool {
        self.screenshots.reviews.visible(hash)
    }

    pub(super) fn screenshot_standing(&self, hash: &Sha256) -> Standing {
        match self.screenshots.reviews.get(hash).map(|s| s.status) {
            Some(review::Status::Pending) => Standing::Hidden,
            Some(review::Status::Removed) => Standing::Blocked,
            _ => Standing::Ok,
        }
    }

    pub(super) fn cleared_image(&self, hash: &Sha256) -> bool {
        self.screenshots
            .reviews
            .get(hash)
            .is_some_and(|s| s.status == review::Status::Approved)
            || self
                .details
                .values()
                .any(|details| details.images.iter().any(|i| i.sha256 == *hash))
            || self
                .packs
                .values()
                .any(|pack| pack.images.iter().any(|i| i.sha256 == *hash))
    }

    pub(super) fn screenshot_targets<'a>(
        &'a self,
        hash: &'a Sha256,
    ) -> impl Iterator<Item = (Target, &'a str)> {
        self.screenshots
            .slots
            .iter()
            .filter_map(move |((target, id), slots)| {
                slots
                    .values()
                    .any(|h| h == hash)
                    .then_some((*target, id.as_str()))
            })
    }

    pub(crate) fn gallery(
        &self,
        target: Target,
        id: &str,
        curated: &[maps::MapImage],
    ) -> Vec<maps::MapImage> {
        let mut seen = BTreeSet::new();
        self.screenshot_hashes(target, id)
            .map(|sha256| maps::MapImage {
                sha256,
                reportable: true,
                caption: None,
                source_url: None,
                credit: None,
                licence: None,
            })
            .chain(curated.iter().cloned())
            .filter(|image| self.servable(&image.sha256) && seen.insert(image.sha256))
            .collect()
    }

    fn authorize_gallery(&self, target: Target, id: &str, xuid: Xuid) -> ApiResult<()> {
        let owner = match target {
            Target::Map => {
                let map = id.parse::<MapId>().map_err(|_| not_found("no such map"))?;
                if let Some(why) = self.refusal(&map) {
                    return Err(gone(why));
                }
                self.maps.get(&map).map(|p| p.author)
            }
            Target::Pack => {
                if !self.standing(Target::Pack, id).is_ok() {
                    return Err(gone("an admin took this pack down"));
                }
                self.mod_pack_owner(id)
            }
            Target::Content => return Err(invalid("content has no gallery")),
        }
        .ok_or_else(|| not_found("no such map or pack with an uploader"))?;
        if owner != xuid || self.uploads_off.contains(&xuid) {
            return Err(forbidden("only the uploader can set this gallery's images"));
        }
        Ok(())
    }
}

fn image_error(error: image::ImageError) -> ApiError {
    match error {
        image::ImageError::Limits(_) => too_large("image exceeds the decoder's limits"),
        _ => invalid("invalid PNG, JPEG or WebP image"),
    }
}

fn decode(bytes: &[u8]) -> ApiResult<DynamicImage> {
    if bytes.len() > MAX_INPUT {
        return Err(too_large("an image upload is at most 5 MiB"));
    }
    let format = image::guess_format(bytes).map_err(image_error)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err(invalid("upload a PNG, JPEG or WebP image"));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODED);
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(image_error)?;
    let (w, h) = decoder.dimensions();
    if w == 0
        || h == 0
        || u64::from(w) * u64::from(h) > MAX_PIXELS
        || decoder.total_bytes() > MAX_DECODED
    {
        return Err(too_large(
            "an image must fit within 4096 pixels per side and 8 megapixels",
        ));
    }
    DynamicImage::from_decoder(decoder).map_err(image_error)
}

fn encode(mut photo: DynamicImage) -> ApiResult<Vec<u8>> {
    if photo.width().max(photo.height()) > DISPLAY_SIDE {
        photo = photo.thumbnail(DISPLAY_SIDE, DISPLAY_SIDE);
    }
    let mut photo = photo.into_rgba8();
    // Analyze and serve the same opaque pixels, regardless of the page background.
    for pixel in photo.pixels_mut() {
        let alpha = u16::from(pixel.0[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
        pixel.0[3] = 255;
    }
    loop {
        let mut out = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut out)
            .encode(
                photo.as_raw(),
                photo.width(),
                photo.height(),
                ExtendedColorType::Rgba8,
            )
            .map_err(image_error)?;
        if out.len() as u64 <= MAX_IMAGE_BYTES {
            return Ok(out);
        }
        let width = (photo.width() * 3 / 4).max(1);
        let height = (photo.height() * 3 / 4).max(1);
        photo = image::imageops::thumbnail(&photo, width, height);
    }
}

impl Live {
    async fn set_screenshot(
        &self,
        headers: &HeaderMap,
        target: Target,
        id: &str,
        kind: &str,
        slot: &str,
        body: Body,
    ) -> ApiResult<Response> {
        let (_, xuid) = self.uploader(bearer(headers))?;
        let slot = Slot::parse(kind, slot)?;
        let id = match target {
            Target::Map => id
                .parse::<MapId>()
                .map_err(|_| not_found("no such map"))?
                .to_string(),
            _ => id.to_owned(),
        };
        self.catalog.read().authorize_gallery(target, &id, xuid)?;
        if slot.kind != Kind::Capture {
            return Err(invalid("capture screenshots in the game before uploading"));
        }
        let proof = proof::read(headers)?;
        let source = proof::verify(self, xuid, &proof)?;
        self.catalog
            .read()
            .authorize_capture_map(target, &id, &proof)?;
        let input = axum::body::to_bytes(body, MAX_INPUT)
            .await
            .map_err(|_| too_large("image upload exceeds 5 MiB or is incomplete"))?;
        if Sha256::of(&input) != proof.digest {
            return Err(invalid("capture digest does not match the uploaded image"));
        }
        let photo = decode(&input)?;
        let output = encode(photo)?;
        let sha256 = Sha256::of(&output);
        let size = output.len() as u64;
        self.meter(
            Some(xuid),
            crate::usage::metric::UP_CONTENT,
            input.len() as u64,
        )
        .await;

        // Run inference outside the write lock. Recheck ownership, quotas and
        // moderation after it returns so a slow classifier cannot undo a report.
        let new = self
            .catalog
            .read()
            .screenshots
            .reviews
            .get(&sha256)
            .is_none();
        let evidence = if new {
            Some(
                self.capture_evidence(proof.clone(), source, &output, xuid)
                    .await,
            )
        } else {
            None
        };
        let _guard = self.content_writes.lock().await;
        self.wait_applied(self.store.head().await?).await?;
        let (known, status, submission, version) = {
            let state = self.catalog.read();
            state.authorize_gallery(target, &id, xuid)?;
            state.authorize_capture_map(target, &id, &proof)?;
            if state.withheld_by_pack(&sha256) || !state.blob_standing(&sha256).is_ok() {
                return Err(gone("an admin took this image down"));
            }
            let known = state.blobs.contains_key(&sha256);
            if !known {
                let limits = &self.policy().content;
                if size > limits.layer_bytes
                    || state.used_by(xuid).saturating_add(size) > limits.quota_bytes
                {
                    return Err(too_large(
                        "image exceeds this server's layer size or account quota",
                    ));
                }
            }
            let reviews = &state.screenshots.reviews;
            let (status, submission) = match reviews.get(&sha256) {
                Some(record) if record.status == review::Status::Removed => {
                    return Err(gone("an admin removed this screenshot"));
                }
                Some(record) => {
                    if record
                        .proof
                        .as_ref()
                        .is_some_and(|old| old.map_id != proof.map_id)
                    {
                        return Err(conflict(
                            "this screenshot is already associated with another map",
                        ));
                    }
                    (record.status, None)
                }
                None => {
                    let evidence =
                        evidence.ok_or_else(|| conflict("screenshot changed, retry the upload"))?;
                    (evidence.status, Some(evidence))
                }
            };
            if state
                .screenshots
                .slots
                .get(&(target, id.clone()))
                .and_then(|s| s.get(&slot))
                == Some(&sha256)
            {
                return Ok(screenshot_response(sha256, self.position(), status));
            }
            (known, status, submission, reviews.version)
        };

        let mut events = Vec::new();
        if !known {
            let meta = crate::blobs::Meta {
                content_type: Some("image/webp".into()),
                ..Default::default()
            };
            self.content_store()?
                .put(&blob_key(&sha256), output.into(), &meta)
                .await
                .map_err(|_| unavailable("the content store cannot write this image"))?;
            events.push(
                self.server_event(
                    maps::content_stream(&sha256),
                    CONTENT_STORED,
                    serde_json::to_value(ContentStored {
                        sha256,
                        size,
                        uploader: xuid,
                    })
                    .expect("serializes"),
                ),
            );
        }
        events.push(
            self.server_event(
                review::STREAM.into(),
                maps::SCREENSHOT_SET,
                serde_json::to_value(ScreenshotSet {
                    id,
                    target,
                    slot,
                    sha256,
                    submission,
                })
                .expect("serializes"),
            ),
        );
        let position = review::append(self, events, version).await?;
        Ok(screenshot_response(sha256, position, status))
    }

    async fn capture_evidence(
        &self,
        mut proof: CaptureProof,
        source: proof::Source,
        output: &[u8],
        uploader: Xuid,
    ) -> review::Submission {
        let nsfw = match self.screenshot_classifier() {
            Some(classifier) => classifier
                .classify(output)
                .await
                .ok()
                .filter(|s| (0.0..=1.0).contains(s)),
            None => None,
        };
        let shape = match self.outline(&proof.map_id.to_string()).await {
            Ok(bytes) if bytes.len() <= 2 * 1024 * 1024 => {
                match (
                    serde_json::from_slice::<live_proto::game::Outline>(&bytes),
                    image::load_from_memory_with_format(output, ImageFormat::WebP),
                ) {
                    (Ok(outline), Ok(image)) => geometry::compare(&image, &proof.camera, &outline),
                    _ => geometry::Match::Unavailable,
                }
            }
            _ => geometry::Match::Unavailable,
        };
        let reason = match (nsfw, shape, source) {
            (Some(score), _, _) if score >= NSFW_REVIEW_SCORE => {
                Some(review::Reason::ClassifierFlag)
            }
            (None, _, _) => Some(review::Reason::ClassifierUnavailable),
            (Some(score), _, _) if score > NSFW_CLEAR_SCORE => {
                Some(review::Reason::ClassifierUncertain)
            }
            (_, geometry::Match::Mismatch, _) => Some(review::Reason::GeometryMismatch),
            (_, geometry::Match::Unavailable, _) => Some(review::Reason::GeometryUnavailable),
            (_, _, proof::Source::Private) => Some(review::Reason::PrivateCapture),
            (_, _, proof::Source::PeerGame) => Some(review::Reason::PeerCapture),
            (_, _, proof::Source::DedicatedGame) => None,
        };
        proof.permit = None;
        review::Submission {
            uploader,
            status: if reason.is_some() {
                review::Status::Pending
            } else {
                review::Status::Approved
            },
            reason,
            nsfw,
            proof: Some(proof),
        }
    }
}

impl CatalogState {
    fn authorize_capture_map(
        &self,
        target: Target,
        id: &str,
        proof: &CaptureProof,
    ) -> ApiResult<()> {
        if self.refusal(&proof.map_id).is_some() {
            return Err(gone("the captured map is unavailable"));
        }
        let map = self
            .maps
            .get(&proof.map_id)
            .ok_or_else(|| forbidden("the captured map is not published"))?;
        if !map.doc.manifest.base.map.eq_ignore_ascii_case(&proof.map) {
            return Err(forbidden(
                "capture map name does not match the published package",
            ));
        }
        let matches = match target {
            Target::Map => id.parse::<MapId>().ok() == Some(proof.map_id),
            Target::Pack => self.mod_pack_id(&proof.map_id).as_deref() == Some(id),
            Target::Content => false,
        };
        if !matches {
            return Err(forbidden(
                "the captured map does not belong to this gallery",
            ));
        }
        Ok(())
    }
}

fn screenshot_response(sha256: Sha256, position: u64, status: review::Status) -> Response {
    crate::http::json(
        StatusCode::OK,
        &json!({ "ok": true, "sha256": sha256, "position": position, "status": status }),
    )
}

pub(crate) async fn put_map(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, kind, slot)): UrlPath<(String, String, String)>,
    req: Request,
) -> ApiResult<Response> {
    live.set_screenshot(&headers, Target::Map, &id, &kind, &slot, req.into_body())
        .await
}

pub(crate) async fn put_pack(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, kind, slot)): UrlPath<(String, String, String)>,
    req: Request,
) -> ApiResult<Response> {
    live.set_screenshot(&headers, Target::Pack, &id, &kind, &slot, req.into_body())
        .await
}

pub(crate) async fn get_pack(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, n)): UrlPath<(String, usize)>,
    Query(query): Query<ImageQuery>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let img = {
        let state = live.catalog.read();
        if !state.standing(Target::Pack, &id).is_ok() {
            return Err(gone("an admin took this pack down"));
        }
        live.mod_pack_in(&state, &id, true)
            .and_then(|p| p.info.images.get(n).map(|i| i.sha256))
            .ok_or_else(|| not_found("no such pack image"))?
    };
    query.check(img)?;
    let (_, bytes) = live.content(&img.to_string()).await?;
    let kind = maps::image_type(&bytes).ok_or_else(|| not_found("no such image"))?;
    Ok(([(header::CONTENT_TYPE, kind)], bytes).into_response())
}

#[derive(Deserialize)]
pub(super) struct ImageQuery {
    v: Option<String>,
}

impl ImageQuery {
    pub(super) fn check(&self, image: Sha256) -> ApiResult<()> {
        if self
            .v
            .as_ref()
            .is_some_and(|v| v.parse::<Sha256>().ok() != Some(image))
        {
            return Err(not_found("the gallery has changed"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    fn normalize(bytes: &[u8]) -> ApiResult<Vec<u8>> {
        encode(decode(bytes)?)
    }

    fn png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(pixels, width, height, ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    #[test]
    fn gallery_decoder_requires_complete_supported_images_and_bounds_the_input() {
        for bad in [
            b"".as_slice(),
            b"\x89PNG\r\n\x1a\n",
            b"\xff\xd8\xff",
            b"RIFF0000WEBP",
            b"GIF89a",
        ] {
            assert_eq!(normalize(bad).unwrap_err().code, ErrorCode::Invalid);
        }
        assert_eq!(
            normalize(&vec![0; MAX_INPUT + 1]).unwrap_err().code,
            ErrorCode::TooLarge
        );
        let wide = png(MAX_SIDE + 1, 1, &vec![0; (MAX_SIDE as usize + 1) * 3]);
        assert_eq!(normalize(&wide).unwrap_err().code, ErrorCode::TooLarge);
    }

    #[test]
    fn jpeg_and_png_are_reencoded_without_metadata_into_size_capped_webp() {
        let (w, h) = (1024, 700);
        // Incompressible RGB exercises the encoded-byte cap independently of dimensions.
        let mut seed = 31u32;
        let noise: Vec<_> = (0..w * h * 3)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&noise[..3], 1, 1, ExtendedColorType::Rgb8)
            .unwrap();
        for mut source in [png(w, h, &noise), jpeg] {
            source.extend_from_slice(b"metadata-to-drop");
            let out = normalize(&source).unwrap();
            assert_eq!(maps::image_type(&out), Some("image/webp"));
            assert!(out.len() <= MAX_IMAGE_BYTES as usize);
            assert!(!out.windows(16).any(|b| b == b"metadata-to-drop"));
            let decoded = image::load_from_memory_with_format(&out, ImageFormat::WebP).unwrap();
            assert!(decoded.width().max(decoded.height()) <= DISPLAY_SIDE);
            assert_eq!(
                normalize(&out).unwrap(),
                out,
                "re-encoding a normalized WebP is stable"
            );
        }
    }
}
