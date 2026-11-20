//! Edge caching for a whole, immutable public TICK selected by LiveHub.
//! The Cache API stores a bounded 200 under a distinct internal URL. Only
//! this module reconstructs the original 206, including its file-relative
//! Content-Range. Mutable visibility and authenticated reads stay with the hub.
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode, header};
use live_proto::film::{
    MAX_UPLOAD_BYTES,
    range::{DATA_OFFSET, MAX_TICK_PACKED},
};

use live_server::caching::IMMUTABLE_FILM_CHUNK as PUBLIC;
const SAVED_RANGE: &str = "x-h2-film-content-range";

struct Candidate {
    key: String,
    etag: String,
    start: u64,
    end: u64,
}

impl Candidate {
    fn request<B>(request: &Request<B>, private: Option<&str>) -> Option<Self> {
        if !matches!(private.map(str::trim), None | Some("" | "0" | "false"))
            || *request.method() != Method::GET
        {
            return None;
        }
        let headers = request.headers();
        if [
            header::AUTHORIZATION,
            header::PROXY_AUTHORIZATION,
            header::COOKIE,
            header::IF_RANGE,
            header::IF_MATCH,
            header::IF_NONE_MATCH,
            header::IF_MODIFIED_SINCE,
            header::IF_UNMODIFIED_SINCE,
            header::CACHE_CONTROL,
            header::PRAGMA,
            header::UPGRADE,
        ]
        .iter()
        .any(|h| headers.contains_key(h))
        {
            return None;
        }
        let uri = request.uri();
        if !matches!(uri.scheme_str(), Some("https" | "http")) || uri.authority().is_none() {
            return None;
        }
        let id = uri
            .path()
            .strip_prefix("/v1/games/")?
            .strip_suffix("/film")?;
        if !live_proto::game::valid_id(id) {
            return None;
        }
        // Only the canonical URL composed by the reader. Any auth marker,
        // duplicate parameter, alternate spelling or suffix stays with Live.
        let (rev, span) = uri.query()?.strip_prefix("rev=")?.split_once("&span=")?;
        if rev.len() != 64 || !rev.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let (first, last) = span.split_once('-')?;
        let number = |s: &str| {
            if s.len() > 1 && s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            s.parse::<u64>().ok()
        };
        let start = number(first)?;
        let end = number(last)?.checked_add(1)?;
        let len = end.checked_sub(start)?;
        if start < DATA_OFFSET
            || end > MAX_UPLOAD_BYTES
            || !(9..=8 + MAX_TICK_PACKED as u64).contains(&len)
        {
            return None;
        }
        let ranges = headers.get_all(header::RANGE);
        let mut ranges = ranges.iter();
        if ranges.next()?.to_str().ok()?.strip_prefix("bytes=")? != span || ranges.next().is_some()
        {
            return None;
        }
        Some(Self {
            key: format!("{uri}&__h2_cache=public-film-v1"),
            etag: format!("\"film-{rev}\""),
            start,
            end,
        })
    }

    fn len(&self) -> usize {
        (self.end - self.start) as usize
    }

    fn approved(&self, status: StatusCode, h: &HeaderMap) -> bool {
        if [
            header::CONTENT_RANGE,
            header::CONTENT_TYPE,
            header::CACHE_CONTROL,
            header::ETAG,
            header::VARY,
        ]
        .iter()
        .any(|name| h.get_all(name).iter().count() != 1)
        {
            return false;
        }
        let Some(total) = h
            .get(header::CONTENT_RANGE)
            .and_then(|r| r.to_str().ok())
            .and_then(|r| r.strip_prefix(&format!("bytes {}-{}/", self.start, self.end - 1)))
            .and_then(|n| n.parse::<u64>().ok())
        else {
            return false;
        };
        status == StatusCode::PARTIAL_CONTENT
            && (self.end..=MAX_UPLOAD_BYTES).contains(&total)
            && h.get(header::CACHE_CONTROL).is_some_and(|v| v == PUBLIC)
            && h.get(header::ETAG)
                .is_some_and(|v| v.as_bytes() == self.etag.as_bytes())
            && h.get(header::CONTENT_TYPE)
                .is_some_and(|v| v == "application/octet-stream")
            && h.get(header::VARY)
                .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"Authorization, Range"))
            && h.get(header::CONTENT_LENGTH).is_none_or(|v| {
                v.to_str().ok().and_then(|v| v.parse::<usize>().ok()) == Some(self.len())
            })
            && !h.contains_key(header::CONTENT_ENCODING)
            && !h.contains_key(header::SET_COOKIE)
    }

    fn stored_headers(&self, status: StatusCode, h: &HeaderMap) -> Option<HeaderMap> {
        if !self.approved(status, h) {
            return None;
        }
        let mut out = HeaderMap::new();
        for name in [
            header::CONTENT_TYPE,
            header::CACHE_CONTROL,
            header::ETAG,
            header::ACCEPT_RANGES,
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            header::X_CONTENT_TYPE_OPTIONS,
        ] {
            if let Some(v) = h.get(&name) {
                out.insert(name, v.clone());
            }
        }
        out.insert(SAVED_RANGE, h.get(header::CONTENT_RANGE)?.clone());
        Some(out)
    }

    fn restore(&self, status: StatusCode, h: &mut HeaderMap) -> Option<StatusCode> {
        if status != StatusCode::OK || h.contains_key(header::CONTENT_RANGE) {
            return None;
        }
        let span = h.remove(SAVED_RANGE)?;
        h.insert(header::CONTENT_RANGE, span);
        h.insert(
            header::VARY,
            HeaderValue::from_static("Authorization, Range"),
        );
        if !self.approved(StatusCode::PARTIAL_CONTENT, h) {
            return None;
        }
        h.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(&self.len().to_string()).ok()?,
        );
        Some(StatusCode::PARTIAL_CONTENT)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) mod edge {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        response::Response,
    };
    use worker::{Cache, Context, Env, HttpRequest};

    pub(crate) struct Film(Candidate);
    impl Film {
        pub(crate) fn from_request(req: &HttpRequest, env: &Env) -> Option<Self> {
            if !req.uri().path().ends_with("/film") {
                return None;
            }
            // Match the hub's variable/secret lookup when policy is changed.
            let private = env
                .var(crate::settings::PRIVATE)
                .map(|v| v.to_string())
                .or_else(|_| env.secret(crate::settings::PRIVATE).map(|v| v.to_string()))
                .ok();
            Candidate::request(req, private.as_deref()).map(Self)
        }
        pub(crate) async fn hit(&self) -> Option<Response> {
            let entry = Cache::default().get(&self.0.key, false).await.ok()??;
            let (mut parts, body) = worker::HttpResponse::try_from(entry).ok()?.into_parts();
            parts.status = self.0.restore(parts.status, &mut parts.headers)?;
            Some(Response::from_parts(parts, Body::new(body)))
        }
        pub(crate) async fn store(
            self,
            response: Response,
            ctx: &Context,
        ) -> worker::Result<Response> {
            let Some(headers) = self.0.stored_headers(response.status(), response.headers()) else {
                return Ok(response);
            };
            let (parts, body) = response.into_parts();
            let bytes = to_bytes(body, self.0.len())
                .await
                .map_err(|_| worker::Error::RustError("invalid film chunk body".into()))?;
            if bytes.len() != self.0.len() {
                return Err(worker::Error::RustError("short film chunk body".into()));
            }
            let copy = bytes.clone();
            ctx.wait_until(async move {
                let mut cached = axum::response::Response::new(Body::from(copy));
                *cached.headers_mut() = headers;
                if let Ok(cached) = worker::Response::try_from(cached) {
                    let _ = Cache::default().put(self.0.key, cached).await;
                }
            });
            Ok(Response::from_parts(parts, Body::from(bytes)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REV: &str = "abababababababababababababababababababababababababababababababab";
    const SPAN: &str = "144000-144127";
    fn url() -> String {
        format!("https://live.test/v1/games/game/film?rev={REV}&span={SPAN}")
    }
    fn request(uri: &str) -> Request<()> {
        Request::get(uri)
            .header(header::RANGE, format!("bytes={SPAN}"))
            .body(())
            .unwrap()
    }

    #[test]
    fn only_exact_unsigned_pinned_reads_enter_the_cache() {
        let url = url();
        let candidate = Candidate::request(&request(&url), None).unwrap();
        assert_eq!(candidate.len(), 128);
        assert!(candidate.key.starts_with(&url));
        let other_url = url.replace(SPAN, "144128-144255");
        let mut other = request(&other_url);
        other.headers_mut().insert(
            header::RANGE,
            HeaderValue::from_static("bytes=144128-144255"),
        );
        assert_ne!(Candidate::request(&other, None).unwrap().key, candidate.key);
        for alternate in [
            url.replace("/game/", "/another/"),
            url.replace("live.test", "other.test"),
            url.replace(REV, &"1".repeat(64)),
        ] {
            assert_ne!(
                Candidate::request(&request(&alternate), None).unwrap().key,
                candidate.key
            );
        }
        for alt in [
            url.replace(SPAN, "144000-144126"),
            url.replace(REV, "bad"),
            format!("{url}&auth=1"),
            format!("{url}&span={SPAN}"),
            url.replace("&span=", "&x="),
            url.replace("/film?", "/frames?"),
            url.replace(SPAN, "-40"),
        ] {
            assert!(Candidate::request(&request(&alt), None).is_none());
        }
        for key in [
            header::AUTHORIZATION,
            header::COOKIE,
            header::IF_RANGE,
            header::CACHE_CONTROL,
        ] {
            let mut req = request(&url);
            req.headers_mut()
                .insert(key, HeaderValue::from_static("present"));
            assert!(Candidate::request(&req, None).is_none());
        }
        let mut req = request(&url);
        req.headers_mut()
            .append(header::RANGE, HeaderValue::from_static("bytes=0-31"));
        assert!(Candidate::request(&req, None).is_none());
        assert!(Candidate::request(&request(&url), Some("true")).is_none());
        assert!(Candidate::request(&request(&url), Some("invalid")).is_none());
        let mut req = request(&url);
        *req.method_mut() = Method::POST;
        assert!(Candidate::request(&req, None).is_none());
    }

    #[test]
    fn restore_only_the_partial_response_the_hub_approved() {
        let key = Candidate::request(&request(&url()), None).unwrap();
        let range = "bytes 144000-144127/999999";
        let h = HeaderMap::from_iter([
            (header::CONTENT_RANGE, HeaderValue::from_static(range)),
            (header::CACHE_CONTROL, HeaderValue::from_static(PUBLIC)),
            (header::ETAG, key.etag.parse().unwrap()),
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (
                header::VARY,
                HeaderValue::from_static("Authorization, Range"),
            ),
            (
                header::ACCESS_CONTROL_ALLOW_ORIGIN,
                HeaderValue::from_static("*"),
            ),
        ]);
        let mut saved = key.stored_headers(StatusCode::PARTIAL_CONTENT, &h).unwrap();
        assert!(!saved.contains_key(header::CONTENT_RANGE));
        assert!(saved.contains_key(SAVED_RANGE));
        assert_eq!(
            key.restore(StatusCode::OK, &mut saved),
            Some(StatusCode::PARTIAL_CONTENT)
        );
        assert_eq!(saved[header::CONTENT_RANGE], range);
        assert_eq!(saved[header::CONTENT_LENGTH], "128");
        assert_eq!(saved[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(saved[header::VARY], "Authorization, Range");
        assert!(!saved.contains_key(SAVED_RANGE));
        for (name, val) in [
            (header::CACHE_CONTROL, "private, no-store"),
            (header::ETAG, "\"other\""),
            (header::CONTENT_RANGE, "bytes 144000-144001/999999"),
            (header::CONTENT_LENGTH, "512"),
            (header::SET_COOKIE, "id=1"),
            (header::CONTENT_ENCODING, "br"),
            (header::VARY, "cookie"),
        ] {
            let mut alt = h.clone();
            alt.insert(name, HeaderValue::from_static(val));
            assert!(
                key.stored_headers(StatusCode::PARTIAL_CONTENT, &alt)
                    .is_none()
            );
        }
        let mut ambiguous = h.clone();
        ambiguous.append(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
        assert!(
            key.stored_headers(StatusCode::PARTIAL_CONTENT, &ambiguous)
                .is_none()
        );
        assert!(key.stored_headers(StatusCode::OK, &h).is_none());
        assert!(key.stored_headers(StatusCode::NOT_FOUND, &h).is_none());
        assert!(key.restore(StatusCode::OK, &mut h.clone()).is_none());
    }
}
