//! An R2 bucket binding as an `object_store::ObjectStore`, so
//! live-server's `BlobStore` (content, cloud saves, published art) runs on
//! it unchanged.
//!
//! What `BlobStore` uses is here: whole and ranged reads, heads, puts
//! (overwrite, create-only, and if-the-ETag-still-matches), deletes and
//! listing, and multipart uploads, which uploads stream into as the
//! request body arrives (`BlobWriter`). Copies and renames are not here:
//! nothing uses them.
//!
//! The binding is JavaScript and not `Send`; a Worker has one thread, so
//! it and its futures are wrapped ([`SendWrapper`]) to meet the trait.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::stream::{self, BoxStream, StreamExt};
use object_store::path::Path;
use object_store::{
    Attribute, AttributeValue, Attributes, CopyOptions, Error, GetOptions, GetRange, GetResult,
    GetResultPayload, ListResult, MultipartUpload, ObjectMeta, ObjectStore, PutMode,
    PutMultipartOptions, PutOptions, PutPayload, PutResult, Result, UploadPart,
};
use send_wrapper::SendWrapper;
use worker::{Bucket, Conditional, HttpMetadata, Object, Range as R2Range, UploadedPart};

const STORE: &str = "R2";

pub struct R2 {
    bucket: Arc<SendWrapper<Bucket>>,
    name: String,
}

impl R2 {
    pub fn new(bucket: Bucket, name: &str) -> Self {
        R2 {
            bucket: Arc::new(SendWrapper::new(bucket)),
            name: name.to_string(),
        }
    }
}

impl fmt::Debug for R2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "R2({})", self.name)
    }
}

impl fmt::Display for R2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r2://{}", self.name)
    }
}

fn generic(e: impl fmt::Display) -> Error {
    Error::Generic {
        store: STORE,
        source: e.to_string().into(),
    }
}

fn not_found(path: &Path) -> Error {
    Error::NotFound {
        path: path.to_string(),
        source: "no such object".into(),
    }
}

fn meta(o: &Object) -> ObjectMeta {
    let millis = o.uploaded().as_millis() as i64;
    ObjectMeta {
        location: Path::from(o.key()),
        last_modified: chrono::DateTime::from_timestamp_millis(millis).unwrap_or_default(),
        size: o.size(),
        e_tag: Some(o.http_etag()),
        version: Some(o.version()),
    }
}

/// The HTTP headers `BlobStore` keeps on an object.
fn http_metadata(attributes: &Attributes) -> HttpMetadata {
    let get = |a: &Attribute| attributes.get(a).map(|v| v.as_ref().to_string());
    HttpMetadata {
        content_type: get(&Attribute::ContentType),
        content_encoding: get(&Attribute::ContentEncoding),
        cache_control: get(&Attribute::CacheControl),
        content_language: get(&Attribute::ContentLanguage),
        content_disposition: get(&Attribute::ContentDisposition),
        cache_expiry: None,
    }
}

fn attributes(o: &Object) -> Attributes {
    let h = o.http_metadata();
    let mut out = Attributes::new();
    for (a, v) in [
        (Attribute::ContentType, h.content_type),
        (Attribute::ContentEncoding, h.content_encoding),
        (Attribute::CacheControl, h.cache_control),
        (Attribute::ContentLanguage, h.content_language),
        (Attribute::ContentDisposition, h.content_disposition),
    ] {
        if let Some(v) = v {
            out.insert(a, AttributeValue::from(v));
        }
    }
    out
}

/// The byte range a read returned, for an object of `size` bytes.
fn returned(range: &Option<GetRange>, size: u64) -> std::ops::Range<u64> {
    match range {
        None => 0..size,
        Some(GetRange::Bounded(r)) => r.start.min(size)..r.end.min(size),
        Some(GetRange::Offset(o)) => (*o).min(size)..size,
        Some(GetRange::Suffix(n)) => size.saturating_sub(*n)..size,
    }
}

impl R2 {
    async fn put(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> Result<PutResult> {
        let key = location.to_string();
        let condition = match &opts.mode {
            PutMode::Overwrite => None,
            // R2 has no if-none-match on put. The hub is the one writer, so
            // a head first is enough for create-only.
            PutMode::Create => {
                if self.bucket.head(&key).await.map_err(generic)?.is_some() {
                    return Err(Error::AlreadyExists {
                        path: key,
                        source: "the object exists".into(),
                    });
                }
                None
            }
            PutMode::Update(v) => Some(Conditional {
                etag_matches: v.e_tag.clone().map(|t| t.trim_matches('"').to_string()),
                ..Conditional::default()
            }),
        };
        let bytes: Vec<u8> = payload.iter().flat_map(|b| b.iter().copied()).collect();
        let mut put = self
            .bucket
            .put(&key, bytes)
            .http_metadata(http_metadata(&opts.attributes));
        if let Some(c) = condition {
            put = put.only_if(c);
        }
        match put.execute().await.map_err(generic)? {
            Some(o) => Ok(PutResult {
                e_tag: Some(o.http_etag()),
                version: Some(o.version()),
                extensions: Default::default(),
            }),
            None => Err(Error::Precondition {
                path: key,
                source: "the object changed".into(),
            }),
        }
    }

    async fn get(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        let key = location.to_string();
        if options.head {
            let o = self
                .bucket
                .head(&key)
                .await
                .map_err(generic)?
                .ok_or_else(|| not_found(location))?;
            let size = o.size();
            return Ok(GetResult {
                payload: GetResultPayload::Stream(stream::empty().boxed()),
                meta: meta(&o),
                range: 0..size,
                attributes: attributes(&o),
                extensions: Default::default(),
            });
        }
        let mut get = self.bucket.get(&key);
        if let Some(r) = &options.range {
            get = get.range(match r {
                GetRange::Bounded(r) => R2Range::OffsetWithLength {
                    offset: r.start,
                    length: r.end - r.start,
                },
                GetRange::Offset(o) => R2Range::OffsetToEnd { offset: *o },
                GetRange::Suffix(n) => R2Range::Suffix { suffix: *n },
            });
        }
        let o = get
            .execute()
            .await
            .map_err(generic)?
            .ok_or_else(|| not_found(location))?;
        let range = returned(&options.range, o.size());
        let (m, a) = (meta(&o), attributes(&o));
        let bytes = match o.body() {
            Some(b) => b.bytes().await.map_err(generic)?,
            None => Vec::new(),
        };
        Ok(GetResult {
            payload: GetResultPayload::Stream(
                stream::once(async { Ok(Bytes::from(bytes)) }).boxed(),
            ),
            meta: m,
            range,
            attributes: a,
            extensions: Default::default(),
        })
    }
}

/// Every object under `prefix`, a page at a time; with a delimiter, the
/// "directories" too.
async fn list_all(bucket: &Bucket, prefix: Option<String>, delimit: bool) -> Result<ListResult> {
    {
        let prefix = prefix.map(|p| format!("{p}/")).filter(|p| p != "/");
        let mut out = ListResult {
            common_prefixes: Vec::new(),
            objects: Vec::new(),
            extensions: Default::default(),
        };
        let mut cursor: Option<String> = None;
        loop {
            let mut list = bucket.list();
            if let Some(p) = &prefix {
                list = list.prefix(p.clone());
            }
            if delimit {
                list = list.delimiter("/");
            }
            if let Some(c) = cursor.take() {
                list = list.cursor(c);
            }
            let page = list.execute().await.map_err(generic)?;
            out.objects.extend(page.objects().iter().map(meta));
            out.common_prefixes.extend(
                page.delimited_prefixes()
                    .into_iter()
                    .map(|p| Path::from(p.trim_end_matches('/'))),
            );
            match (page.truncated(), page.cursor()) {
                (true, Some(c)) => cursor = Some(c),
                _ => return Ok(out),
            }
        }
    }
}

/// An R2 multipart upload: parts numbered as they are handed in, and
/// completed in number order.
struct Upload {
    upload: RefCell<Option<Rc<worker::MultipartUpload>>>,
    parts: RefCell<Vec<UploadedPart>>,
    next: Cell<u16>,
}

struct R2Upload(Arc<SendWrapper<Upload>>);

impl fmt::Debug for R2Upload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "R2Upload({} parts)", self.0.next.get() - 1)
    }
}

fn finished() -> Error {
    generic("the multipart upload is already finished")
}

#[async_trait]
impl MultipartUpload for R2Upload {
    fn put_part(&mut self, data: PutPayload) -> UploadPart {
        let state = self.0.clone();
        let n = state.next.get();
        state.next.set(n + 1);
        Box::pin(SendWrapper::new(async move {
            let bytes: Vec<u8> = data.iter().flat_map(|b| b.iter().copied()).collect();
            let upload = state.upload.borrow().clone().ok_or_else(finished)?;
            let part = upload.upload_part(n, bytes).await.map_err(generic)?;
            state.parts.borrow_mut().push(part);
            Ok(())
        }))
    }

    async fn complete(&mut self) -> Result<PutResult> {
        let state = self.0.clone();
        SendWrapper::new(async move {
            let upload = state.upload.borrow_mut().take().ok_or_else(finished)?;
            // Every part has been awaited by now, so this is the only handle.
            let upload =
                Rc::try_unwrap(upload).map_err(|_| generic("a part is still uploading"))?;
            let mut parts = std::mem::take(&mut *state.parts.borrow_mut());
            parts.sort_by_key(UploadedPart::part_number);
            let o = upload.complete(parts).await.map_err(generic)?;
            Ok(PutResult {
                e_tag: Some(o.http_etag()),
                version: Some(o.version()),
                extensions: Default::default(),
            })
        })
        .await
    }

    async fn abort(&mut self) -> Result<()> {
        let state = self.0.clone();
        SendWrapper::new(async move {
            let upload = state.upload.borrow_mut().take().ok_or_else(finished)?;
            upload.abort().await.map_err(generic)
        })
        .await
    }
}

#[async_trait]
impl ObjectStore for R2 {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> Result<PutResult> {
        SendWrapper::new(self.put(location, payload, opts)).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> Result<Box<dyn MultipartUpload>> {
        let bucket = self.bucket.clone();
        let key = location.to_string();
        let upload = SendWrapper::new(async move {
            bucket
                .create_multipart_upload(key)
                .http_metadata(http_metadata(&opts.attributes))
                .execute()
                .await
                .map_err(generic)
        })
        .await?;
        Ok(Box::new(R2Upload(Arc::new(SendWrapper::new(Upload {
            upload: RefCell::new(Some(Rc::new(upload))),
            parts: RefCell::new(Vec::new()),
            next: Cell::new(1),
        })))))
    }

    async fn get_opts(&self, location: &Path, options: GetOptions) -> Result<GetResult> {
        SendWrapper::new(self.get(location, options)).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, Result<Path>>,
    ) -> BoxStream<'static, Result<Path>> {
        let bucket = Arc::clone(&self.bucket);
        locations
            .then(move |p| {
                let bucket = bucket.clone();
                SendWrapper::new(async move {
                    let p = p?;
                    bucket.delete(p.to_string()).await.map_err(generic)?;
                    Ok(p)
                })
            })
            .boxed()
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, Result<ObjectMeta>> {
        let bucket = Arc::clone(&self.bucket);
        let prefix = prefix.map(Path::to_string);
        stream::once(SendWrapper::new(async move {
            list_all(&bucket, prefix, false).await
        }))
        .flat_map(|listed| match listed {
            Ok(l) => stream::iter(l.objects.into_iter().map(Ok)).boxed(),
            Err(e) => stream::once(async { Err(e) }).boxed(),
        })
        .boxed()
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> Result<ListResult> {
        let prefix = prefix.map(Path::to_string);
        SendWrapper::new(list_all(&self.bucket, prefix, true)).await
    }

    async fn copy_opts(&self, _from: &Path, _to: &Path, _options: CopyOptions) -> Result<()> {
        Err(Error::NotImplemented {
            operation: "copy within R2 from a Worker".into(),
            implementer: STORE.into(),
        })
    }
}
