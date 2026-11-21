//! Render published game-art emblems at the edge without waking the hub.

use axum::body::Body;
use axum::response::Response;
use futures_util::StreamExt;
use http::{Method, StatusCode, header};
use live_server::{art, caching, emblem};
use worker::{Cache, Context, Fetch, HttpRequest, Url};

pub async fn image(req: &HttpRequest, base: Option<&str>, ctx: &Context) -> Response {
    let path = req.uri().path();
    let missing = || emblem::error(StatusCode::NOT_FOUND);
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        let mut response = emblem::error(StatusCode::METHOD_NOT_ALLOWED);
        response
            .headers_mut()
            .insert(header::ALLOW, http::HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    let (Some(spec), Ok(requested)) = (
        emblem::Spec::parse(path),
        emblem::query_revision(req.uri().query()),
    ) else {
        return emblem::error(StatusCode::BAD_REQUEST);
    };
    let Some(base) = base
        .map(str::trim)
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty())
    else {
        return missing();
    };

    // The configured R2 directory is published at a new, immutable URL
    // for each extraction.
    let version = emblem::revision(&[base.as_bytes()]);
    match requested {
        None => return emblem::redirect(path, &version, caching::REVALIDATE),
        Some(v) if v != version => return missing(),
        _ => {}
    }
    let head = *req.method() == Method::HEAD;
    // A request that bears a token never reads from or fills the shared
    // cache, so a cache key holds no auth (docs/live/live-api.md, Caching).
    let shared = !req.headers().contains_key(header::AUTHORIZATION);
    let cache = Cache::default();
    let key = req.uri().to_string();
    if shared
        && let Ok(Some(cached)) = cache.get(&key, false).await
        && let Ok(cached) = worker::HttpResponse::try_from(cached)
    {
        return cached.map(|body| if head { Body::empty() } else { Body::new(body) });
    }

    let Some(manifest) = read(base, art::MANIFEST, emblem::MAX_MANIFEST_BYTES)
        .await
        .and_then(|bytes| emblem::ArtManifest::parse(&bytes))
    else {
        return missing();
    };
    let Some(layers) = manifest.layers(&spec) else {
        return missing();
    };
    let (Some(bg), Some(fg)) = futures_util::join!(
        read(base, layers.bg, emblem::MAX_MASK_BYTES),
        read(base, layers.fg, emblem::MAX_MASK_BYTES),
    ) else {
        return missing();
    };
    let Ok(png) = emblem::compose_png(&bg, &fg, layers.colours()) else {
        return missing();
    };
    if shared
        && let Ok(mut cached) = worker::Response::from_bytes(png.clone())
    {
        let _ = cached.headers_mut().set("Content-Type", "image/png");
        let _ = cached
            .headers_mut()
            .set("Cache-Control", art::PUBLISHED_CACHE);
        ctx.wait_until(async move {
            let _ = cache.put(key, cached).await;
        });
    }
    let mut response = emblem::png_response(png, art::PUBLISHED_CACHE);
    if head {
        *response.body_mut() = Body::empty();
    }
    response
}

async fn read(base: &str, file: &str, limit: usize) -> Option<Vec<u8>> {
    let url = Url::parse(&format!("{base}/{file}")).ok()?;
    let mut response = Fetch::Url(url).send().await.ok()?;
    if response.status_code() != 200 {
        return None;
    }
    let mut stream = response.stream().ok()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if chunk.len() > limit - bytes.len() {
            return None;
        }
        bytes.extend(chunk);
    }
    Some(bytes)
}
