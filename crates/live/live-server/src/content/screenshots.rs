//! Uploader-owned gallery slots. A slot update and any new content record
//! share one append, so replay sees the same gallery as the publishing node.
use super::*;
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageFormat, ImageReader, Limits};
use std::io::Cursor;

const MAX_INPUT: usize = 5 * 1024 * 1024;
const MAX_SIDE: u32 = 4096;
const MAX_PIXELS: u64 = 8_000_000;
const MAX_DECODED: u64 = 32 * 1024 * 1024;
const DISPLAY_SIDE: u32 = 1280;

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
}

#[derive(Debug, Default)]
pub(super) struct Galleries(BTreeMap<(Target, String), BTreeMap<Slot, Sha256>>);

impl CatalogState {
    pub(super) fn apply_screenshot(&mut self, data: &serde_json::Value) {
        if let Ok(event) = serde_json::from_value::<ScreenshotSet>(data.clone()) {
            self.screenshots
                .0
                .entry((event.target, event.id))
                .or_default()
                .insert(event.slot, event.sha256);
        }
    }

    pub(crate) fn screenshot_hashes(
        &self,
        target: Target,
        id: &str,
    ) -> impl Iterator<Item = Sha256> + '_ {
        self.screenshots
            .0
            .get(&(target, id.to_owned()))
            .into_iter()
            .flat_map(|slots| slots.values().copied())
    }

    pub(crate) fn screenshot_targets<'a>(
        &'a self,
        hash: &'a Sha256,
    ) -> impl Iterator<Item = (Target, &'a str)> {
        self.screenshots
            .0
            .iter()
            .filter_map(move |((target, id), slots)| {
                slots
                    .values()
                    .any(|h| h == hash)
                    .then_some((*target, id.as_str()))
            })
    }

    /// Display indices are dense. Stable upload slots are independent of
    /// index so a capture retry never displaces an uploader's cover.
    pub(crate) fn gallery(
        &self,
        target: Target,
        id: &str,
        curated: &[maps::MapImage],
    ) -> Vec<maps::MapImage> {
        let mut seen = BTreeSet::new();
        self.screenshot_hashes(target, id)
            .filter(|h| self.blob_standing(h).is_ok())
            .map(|sha256| maps::MapImage {
                sha256,
                caption: None,
                source_url: None,
                credit: None,
                licence: None,
            })
            .chain(curated.iter().cloned())
            .filter(|image| seen.insert(image.sha256))
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

fn normalize(bytes: &[u8]) -> ApiResult<Vec<u8>> {
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
    let mut photo = DynamicImage::from_decoder(decoder).map_err(image_error)?;
    if w.max(h) > DISPLAY_SIDE {
        photo = photo.thumbnail(DISPLAY_SIDE, DISPLAY_SIDE);
    }
    let mut photo = photo.into_rgba8();
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
        bearer: Option<&str>,
        target: Target,
        id: &str,
        kind: &str,
        slot: &str,
        body: Body,
    ) -> ApiResult<Response> {
        let (_, xuid) = self.uploader(bearer)?;
        let slot = Slot::parse(kind, slot)?;
        // Map ids are SHA-256 values. Use the canonical spelling everywhere,
        // including the gallery key and the server-signed event stream.
        let id = match target {
            Target::Map => id
                .parse::<MapId>()
                .map_err(|_| not_found("no such map"))?
                .to_string(),
            _ => id.to_owned(),
        };
        self.catalog.read().authorize_gallery(target, &id, xuid)?;
        let input = axum::body::to_bytes(body, MAX_INPUT)
            .await
            .map_err(|_| too_large("image upload exceeds 5 MiB or is incomplete"))?;
        let output = normalize(&input)?;
        let sha256 = Sha256::of(&output);
        let size = output.len() as u64;
        self.meter(
            Some(xuid),
            crate::usage::metric::UP_CONTENT,
            input.len() as u64,
        )
        .await;
        let _guard = self.content_writes.lock().await;
        let known = {
            let state = self.catalog.read();
            state.authorize_gallery(target, &id, xuid)?;
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
            } else if state
                .screenshots
                .0
                .get(&(target, id.clone()))
                .and_then(|s| s.get(&slot))
                == Some(&sha256)
            {
                return Ok(screenshot_response(sha256, self.position()));
            }
            known
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
        let stream = match target {
            Target::Map => format!("package-{id}"),
            Target::Pack => maps::pack_stream(&id),
            Target::Content => unreachable!("galleries belong to maps and packs"),
        };
        events.push(
            self.server_event(
                stream,
                maps::SCREENSHOT_SET,
                serde_json::to_value(ScreenshotSet {
                    id,
                    target,
                    slot,
                    sha256,
                })
                .expect("serializes"),
            ),
        );
        let recorded = self.store.append(events, &[]).await?;
        let position = recorded.last().expect("screenshot event").position;
        self.wait_applied(position).await?;
        Ok(screenshot_response(sha256, position))
    }
}

fn screenshot_response(sha256: Sha256, position: u64) -> Response {
    crate::http::json(
        StatusCode::OK,
        &json!({ "ok": true, "sha256": sha256, "position": position }),
    )
}

pub(crate) async fn put_map(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, kind, slot)): UrlPath<(String, String, String)>,
    req: Request,
) -> ApiResult<Response> {
    live.set_screenshot(
        bearer(&headers),
        Target::Map,
        &id,
        &kind,
        &slot,
        req.into_body(),
    )
    .await
}

pub(crate) async fn put_pack(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, kind, slot)): UrlPath<(String, String, String)>,
    req: Request,
) -> ApiResult<Response> {
    live.set_screenshot(
        bearer(&headers),
        Target::Pack,
        &id,
        &kind,
        &slot,
        req.into_body(),
    )
    .await
}

pub(crate) async fn get_pack(
    State(live): Shared,
    headers: HeaderMap,
    UrlPath((id, n)): UrlPath<(String, usize)>,
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
    let (_, bytes) = live.content(&img.to_string()).await?;
    let kind = maps::image_type(&bytes).ok_or_else(|| not_found("no such image"))?;
    Ok(([(header::CONTENT_TYPE, kind)], bytes).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

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
