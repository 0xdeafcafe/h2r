//! Where the server keeps bytes that are not events: custom content,
//! cloud saves and their indexes, and (when published) game art. One
//! interface, two places (docs/live/live-hosting.md, "Free hosting"):
//!
//! - **The data directory** (the default): `<data>/content/<aa>/<sha256>`,
//!   `<data>/storage/<xuid>/...`, the layout servers always had.
//! - **An S3-compatible bucket**, made for Cloudflare R2 (`s3://BUCKET`,
//!   endpoint and keys from the environment, region `auto`).
//!
//! Built on the `object_store` crate (Apache Arrow's): its `ObjectStore` trait
//! is the abstraction (put, get, ranged get, head, delete, list, multipart
//! uploads, conditional puts, content type and encoding), with a local
//! directory, an in-memory store for tests and S3 behind it, so this module
//! only adds our key layout, our errors and the public URL.
//!
//! A server has up to two: [`Blobs::private`] for everything, served only
//! through the server, and optionally [`Blobs::public`], a bucket the world
//! reads directly at a public URL (R2's custom domain or r2.dev), for what
//! anyone may fetch anyway (shared game art, published content) so those
//! bytes skip the server and its tunnel. Never one bucket for both: a
//! public R2 bucket is public as a whole, and a cloud save must not be
//! fetchable by anyone who knows its hash.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
#[cfg(feature = "native")]
use object_store::aws::AmazonS3Builder;
#[cfg(feature = "native")]
use object_store::local::LocalFileSystem;
use object_store::memory::InMemory;
use object_store::path::Path as Key;
use object_store::prefix::PrefixStore;
use object_store::{
    Attribute, Attributes, GetOptions, GetRange, ObjectStore, ObjectStoreExt, PutMode, PutOptions,
    PutPayload, UpdateVersion,
};

/// The key prefixes that hold the server's blobs, which `h2 live store copy`
/// moves. Nothing else in the data directory (the key, the database) is a
/// blob.
pub const PREFIXES: &[&str] = &[
    "content/",
    "storage/",
    "art/",
    FRAMES_PREFIX,
    crate::sim_runs::REPORT_PREFIX,
];

/// Recorded games' frame chunks: `frames/<game id>/<first tick>`
/// (crate::games::chunk_key).
pub const FRAMES_PREFIX: &str = "frames/";

/// Environment variables for an `s3://` store. The `_PUBLIC_` forms
/// configure the public one, falling back to these.
pub const ENV_ENDPOINT: &str = "H2_LIVE_S3_ENDPOINT";
pub const ENV_ACCESS_KEY: &str = "H2_LIVE_S3_ACCESS_KEY_ID";
pub const ENV_SECRET_KEY: &str = "H2_LIVE_S3_SECRET_ACCESS_KEY";
pub const ENV_REGION: &str = "H2_LIVE_S3_REGION";

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("{0}")]
    Store(#[from] object_store::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Config(String),
    /// A conditional write lost to another writer.
    #[error("{0} changed while it was being written")]
    Changed(String),
}

pub type Result<T, E = BlobError> = std::result::Result<T, E>;

/// How an object is to be served: its type, encoding and cache policy.
/// Kept only where the store can (S3; a directory has no metadata, and the
/// server works these out itself when it serves from one).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub content_type: Option<String>,
    pub content_encoding: Option<String>,
    pub cache_control: Option<String>,
}

impl Meta {
    fn attributes(&self) -> Attributes {
        let mut a = Attributes::new();
        for (k, v) in [
            (Attribute::ContentType, &self.content_type),
            (Attribute::ContentEncoding, &self.content_encoding),
            (Attribute::CacheControl, &self.cache_control),
        ] {
            if let Some(v) = v {
                a.insert(k, v.clone().into());
            }
        }
        a
    }
}

/// An object's size and identity, from [`BlobStore::head`] or a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub key: String,
    pub size: u64,
    pub e_tag: Option<String>,
}

/// Which write of an object a reader saw, for [`BlobStore::put_if`]: `None`
/// when it did not exist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen(Option<(Option<String>, Option<String>)>);

/// One place blobs live.
#[derive(Clone)]
pub struct BlobStore {
    store: Arc<dyn ObjectStore>,
    /// A directory's root (a write renames its temporary file into place,
    /// and no metadata is kept), or `None`.
    root: Option<PathBuf>,
    /// For messages: the directory, `s3://bucket/prefix` or `memory`.
    describe: String,
    /// Where the world reads this store's objects: `<url>/<key>`.
    public_url: Option<String>,
}

impl std::fmt::Debug for BlobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.describe)
    }
}

/// What `--blobs`, `--public-blobs` and `--from-blobs`/`--to-blobs` take:
/// a directory (`file:DIR` or a plain path), `s3://BUCKET[/PREFIX]`, or
/// `memory:` (tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Dir(PathBuf),
    S3 { bucket: String, prefix: String },
    Memory,
}

impl Location {
    pub fn parse(s: &str) -> Result<Self> {
        if s == "memory:" {
            return Ok(Location::Memory);
        }
        if let Some(rest) = s.strip_prefix("s3://") {
            let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
            if bucket.is_empty() {
                return Err(BlobError::Config(format!("{s:?} names no bucket")));
            }
            return Ok(Location::S3 {
                bucket: bucket.to_string(),
                prefix: prefix.trim_matches('/').to_string(),
            });
        }
        if s.contains("://") {
            return Err(BlobError::Config(format!(
                "{s:?}: blobs go in a directory or s3://BUCKET"
            )));
        }
        let path = s.strip_prefix("file:").unwrap_or(s);
        if path.is_empty() {
            return Err(BlobError::Config("an empty blob directory".into()));
        }
        Ok(Location::Dir(PathBuf::from(path)))
    }
}

/// An S3 store's connection, from the environment ([`ENV_ENDPOINT`] and
/// friends; the public store reads `H2_LIVE_S3_PUBLIC_*` first).
#[derive(Clone, PartialEq, Eq)]
pub struct S3Config {
    pub endpoint: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
}

impl std::fmt::Debug for S3Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl S3Config {
    /// One setting (`name` is the plain `H2_LIVE_S3_*` form), or `None` if
    /// unset or blank.
    pub fn from_env_one(
        env: &dyn Fn(&str) -> Option<String>,
        name: &str,
        public: bool,
    ) -> Option<String> {
        let public_name = name.replacen("H2_LIVE_S3_", "H2_LIVE_S3_PUBLIC_", 1);
        public
            .then(|| env(&public_name))
            .flatten()
            .or_else(|| env(name))
            .filter(|v| !v.trim().is_empty())
    }

    /// `public`: read `H2_LIVE_S3_PUBLIC_<NAME>` before `H2_LIVE_S3_<NAME>`.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>, public: bool) -> Result<Self> {
        let get = |name: &str| Self::from_env_one(env, name, public);
        let need = |name: &str| {
            get(name).ok_or_else(|| BlobError::Config(format!("an s3:// blob store needs ${name}")))
        };
        Ok(S3Config {
            endpoint: need(ENV_ENDPOINT)?,
            access_key_id: need(ENV_ACCESS_KEY)?,
            secret_access_key: need(ENV_SECRET_KEY)?,
            region: get(ENV_REGION).unwrap_or_else(|| "auto".into()),
        })
    }
}

impl BlobStore {
    #[cfg(feature = "native")]
    /// `root` (created if missing): today's data-directory layout.
    pub fn dir(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        let store = LocalFileSystem::new_with_prefix(root)?;
        Ok(BlobStore {
            store: Arc::new(store),
            root: Some(root.to_path_buf()),
            describe: root.display().to_string(),
            public_url: None,
        })
    }

    /// Nothing kept (tests).
    pub fn memory() -> Self {
        BlobStore {
            store: Arc::new(InMemory::new()),
            root: None,
            describe: "memory".into(),
            public_url: None,
        }
    }

    #[cfg(feature = "native")]
    /// A bucket at `config`'s endpoint (R2: `https://<account id>.r2.
    /// cloudflarestorage.com`), keys under `prefix`. Path-style requests,
    /// conditional puts by ETag (R2 and MinIO take If-Match/If-None-Match).
    pub fn s3(bucket: &str, prefix: &str, config: &S3Config) -> Result<Self> {
        // The HTTP client's TLS runs on rustls with ring, the server's own
        // provider; it needs to be the process default (once is enough).
        let _ = rustls::crypto::ring::default_provider().install_default();
        let s3 = AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_endpoint(config.endpoint.trim_end_matches('/'))
            .with_region(&config.region)
            .with_access_key_id(&config.access_key_id)
            .with_secret_access_key(&config.secret_access_key)
            .with_allow_http(config.endpoint.starts_with("http://"))
            .with_virtual_hosted_style_request(false)
            .build()?;
        let describe = if prefix.is_empty() {
            format!("s3://{bucket}")
        } else {
            format!("s3://{bucket}/{prefix}")
        };
        let store: Arc<dyn ObjectStore> = if prefix.is_empty() {
            Arc::new(s3)
        } else {
            Arc::new(PrefixStore::new(s3, prefix))
        };
        Ok(BlobStore {
            store,
            root: None,
            describe,
            public_url: None,
        })
    }

    #[cfg(feature = "native")]
    /// Opens `location`; `public` picks the public store's S3 settings.
    pub fn open(
        location: &Location,
        env: &dyn Fn(&str) -> Option<String>,
        public: bool,
    ) -> Result<Self> {
        match location {
            Location::Dir(root) => Self::dir(root),
            Location::Memory => Ok(Self::memory()),
            Location::S3 { bucket, prefix } => {
                Self::s3(bucket, prefix, &S3Config::from_env(env, public)?)
            }
        }
    }

    /// Any other object store, such as the Worker's R2 bucket binding
    /// (crates/live-worker), described as `describe` in messages.
    pub fn from_store(store: Arc<dyn ObjectStore>, describe: &str) -> Self {
        BlobStore {
            store,
            root: None,
            describe: describe.into(),
            public_url: None,
        }
    }

    /// This store's objects are readable by anyone at `<url>/<key>`.
    pub fn with_public_url(mut self, url: &str) -> Self {
        self.public_url = Some(url.trim_end_matches('/').to_string());
        self
    }

    pub fn describe(&self) -> &str {
        &self.describe
    }

    /// Whether this is a directory on this machine.
    pub fn is_dir(&self) -> bool {
        self.root.is_some()
    }

    /// The public address of `key`, if this store is public. Each segment
    /// is percent-encoded as a URL path segment.
    pub fn public_url(&self, key: &str) -> Option<String> {
        let base = self.public_url.as_ref()?;
        let path: Vec<String> = key.split('/').map(encode_segment).collect();
        Some(format!("{base}/{}", path.join("/")))
    }

    fn key(key: &str) -> Result<Key> {
        Key::parse(key).map_err(|e| BlobError::Config(format!("bad blob key {key:?}: {e}")))
    }

    fn options(&self, meta: &Meta, mode: PutMode) -> PutOptions {
        PutOptions {
            mode,
            // A directory keeps no metadata (and refuses to be given any).
            attributes: if self.is_dir() {
                Attributes::new()
            } else {
                meta.attributes()
            },
            ..PutOptions::default()
        }
    }

    pub async fn put(&self, key: &str, bytes: Bytes, meta: &Meta) -> Result<()> {
        self.store
            .put_opts(
                &Self::key(key)?,
                PutPayload::from(bytes),
                self.options(meta, PutMode::Overwrite),
            )
            .await?;
        Ok(())
    }

    /// Stores the file at `file` as `key`. In a directory on the same disk
    /// the file is renamed into place (it is gone afterwards); otherwise it
    /// is uploaded in parts as it is read, and left for the caller to
    /// remove.
    pub async fn put_file(&self, key: &str, file: &Path, meta: &Meta) -> Result<()> {
        let k = Self::key(key)?;
        if let Some(root) = &self.root {
            let dest = root.join(key);
            if let Some(dir) = dest.parent() {
                std::fs::create_dir_all(dir)?;
            }
            if std::fs::rename(file, &dest).is_ok() {
                return Ok(());
            }
        }
        #[cfg(not(feature = "native"))]
        return {
            let _ = (k, meta);
            Err(BlobError::Config(
                "no files to upload from on a Worker: put the bytes".into(),
            ))
        };
        #[cfg(feature = "native")]
        self.put_file_parts(k, file, meta).await
    }

    /// A writer for `key` that takes the object as it arrives: an upload
    /// streams into it with no temporary file. Nothing shows under `key`
    /// until [`BlobWriter::finish`]; [`BlobWriter::abort`] leaves the key
    /// as it was.
    pub fn writer(&self, key: &str, meta: &Meta) -> Result<BlobWriter> {
        Ok(BlobWriter {
            store: self.store.clone(),
            key: Self::key(key)?,
            options: self.options(meta, PutMode::Overwrite),
            buf: Vec::new(),
            upload: None,
            size: 0,
        })
    }

    /// [`BlobStore::put_file`] to an object store: in parts, as it is read.
    #[cfg(feature = "native")]
    async fn put_file_parts(&self, k: Key, file: &Path, meta: &Meta) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut writer = object_store::buffered::BufWriter::new(self.store.clone(), k);
        if !self.is_dir() {
            writer = writer.with_attributes(meta.attributes());
        }
        let mut f = tokio::fs::File::open(file).await?;
        let mut buf = vec![0u8; 1 << 20];
        let read = async {
            loop {
                let n = f.read(&mut buf).await?;
                if n == 0 {
                    return Ok::<_, BlobError>(());
                }
                writer.put(Bytes::copy_from_slice(&buf[..n])).await?;
            }
        }
        .await;
        if let Err(e) = read {
            // Not shut down yet: drop any parts already uploaded.
            let _ = writer.abort().await;
            return Err(e);
        }
        writer.shutdown().await?;
        Ok(())
    }

    /// The whole object; `None` if there is none.
    pub async fn get(&self, key: &str) -> Result<Option<Bytes>> {
        Ok(self.get_seen(key).await?.map(|(b, _)| b))
    }

    /// The object and which write of it this is ([`Self::put_if`]).
    pub async fn get_seen(&self, key: &str) -> Result<Option<(Bytes, Seen)>> {
        match self.store.get(&Self::key(key)?).await {
            Ok(r) => {
                let seen = Seen(Some((r.meta.e_tag.clone(), r.meta.version.clone())));
                Ok(Some((r.bytes().await?, seen)))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The object as a stream, with its size; `None` if there is none.
    pub async fn get_stream(
        &self,
        key: &str,
    ) -> Result<Option<(u64, BoxStream<'static, Result<Bytes>>)>> {
        match self.store.get(&Self::key(key)?).await {
            Ok(r) => {
                let size = r.meta.size;
                let stream = r.into_stream().map(|c| c.map_err(BlobError::from)).boxed();
                Ok(Some((size, stream)))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Up to the first `n` bytes; `None` if there is no object.
    pub async fn get_prefix(&self, key: &str, n: u64) -> Result<Option<Bytes>> {
        let options = GetOptions {
            range: Some(GetRange::Bounded(0..n)),
            ..GetOptions::default()
        };
        match self.store.get_opts(&Self::key(key)?, options).await {
            Ok(r) => Ok(Some(r.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// How the object is to be served, as the store kept it; `None` if there
    /// is no object. A directory keeps none, so all of it is `None`.
    pub async fn meta(&self, key: &str) -> Result<Option<Meta>> {
        let options = GetOptions {
            head: true,
            ..GetOptions::default()
        };
        match self.store.get_opts(&Self::key(key)?, options).await {
            Ok(r) => {
                let get = |a: Attribute| r.attributes.get(&a).map(|v| v.as_ref().to_string());
                Ok(Some(Meta {
                    content_type: get(Attribute::ContentType),
                    content_encoding: get(Attribute::ContentEncoding),
                    cache_control: get(Attribute::CacheControl),
                }))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn head(&self, key: &str) -> Result<Option<Info>> {
        match self.store.head(&Self::key(key)?).await {
            Ok(m) => Ok(Some(Info {
                key: key.to_string(),
                size: m.size,
                e_tag: m.e_tag,
            })),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Removes `key`; nothing there is fine.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self.store.delete(&Self::key(key)?).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Every object whose key starts with `prefix` (a whole directory: end
    /// it with `/`), in key order.
    pub async fn list(&self, prefix: &str) -> Result<Vec<Info>> {
        let dir = prefix.trim_end_matches('/');
        let p = (!dir.is_empty()).then(|| Self::key(dir)).transpose()?;
        let mut out = Vec::new();
        let mut stream = self.store.list(p.as_ref());
        while let Some(m) = stream.next().await {
            let m = match m {
                Ok(m) => m,
                Err(object_store::Error::NotFound { .. }) => continue,
                Err(e) => return Err(e.into()),
            };
            let key = m.location.to_string();
            if key.starts_with(prefix) || format!("{key}/").starts_with(prefix) {
                out.push(Info {
                    key,
                    size: m.size,
                    e_tag: m.e_tag,
                });
            }
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }

    /// Writes `key` only if it is still the write `seen` (or, for a
    /// [`Seen`] of nothing, still absent): [`BlobError::Changed`] otherwise.
    /// A store without conditional writes (a directory, which one server
    /// owns) overwrites.
    pub async fn put_if(&self, key: &str, bytes: Bytes, seen: &Seen, meta: &Meta) -> Result<Seen> {
        let k = Self::key(key)?;
        let mode = match &seen.0 {
            None => PutMode::Create,
            Some((e_tag, version)) => PutMode::Update(UpdateVersion {
                e_tag: e_tag.clone(),
                version: version.clone(),
            }),
        };
        let attempt = self
            .store
            .put_opts(
                &k,
                PutPayload::from(bytes.clone()),
                self.options(meta, mode),
            )
            .await;
        let done = match attempt {
            Ok(r) => r,
            Err(object_store::Error::NotImplemented { .. }) => {
                self.store
                    .put_opts(
                        &k,
                        PutPayload::from(bytes),
                        self.options(meta, PutMode::Overwrite),
                    )
                    .await?
            }
            Err(
                object_store::Error::Precondition { .. }
                | object_store::Error::AlreadyExists { .. },
            ) => {
                return Err(BlobError::Changed(key.to_string()));
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Seen(Some((done.e_tag, done.version))))
    }
}

/// A URL path segment: unreserved characters as they are, the rest
/// percent-encoded.
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The server's blob stores: the private one, always, and a public one when
/// configured.
#[derive(Clone, Debug)]
pub struct Blobs {
    pub private: BlobStore,
    pub public: Option<BlobStore>,
}

impl Blobs {
    #[cfg(feature = "native")]
    /// The data directory, as servers always kept blobs.
    pub fn dir(data_dir: &Path) -> Result<Self> {
        Ok(Blobs {
            private: BlobStore::dir(data_dir)?,
            public: None,
        })
    }

    #[cfg(feature = "native")]
    /// `--blobs` (default: the data directory), `--public-blobs` with
    /// `--public-blobs-url`. A public store needs its URL: without one there
    /// is nothing to send clients to.
    pub fn configure(
        data_dir: &Path,
        private: Option<&str>,
        public: Option<&str>,
        public_url: Option<&str>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self> {
        let private = match private {
            Some(p) => BlobStore::open(&Location::parse(p)?, env, false)?,
            None => BlobStore::dir(data_dir)?,
        };
        let public = match (public, public_url) {
            (Some(p), Some(url)) => {
                check_public_url(url)?;
                Some(BlobStore::open(&Location::parse(p)?, env, true)?.with_public_url(url))
            }
            (None, None) => None,
            (Some(_), None) => {
                return Err(BlobError::Config(
                    "--public-blobs needs --public-blobs-url (the bucket's public address: \
                     its custom domain or r2.dev URL)"
                        .into(),
                ));
            }
            (None, Some(_)) => {
                return Err(BlobError::Config(
                    "--public-blobs-url goes with --public-blobs (the bucket it serves)".into(),
                ));
            }
        };
        Ok(Blobs { private, public })
    }
}

#[cfg(feature = "native")]
pub(crate) fn check_public_url(url: &str) -> Result<()> {
    let ok = (url.starts_with("https://") || url.starts_with("http://"))
        && !url.contains('?')
        && !url.contains('#');
    if ok {
        Ok(())
    } else {
        Err(BlobError::Config(format!(
            "--public-blobs-url wants https://HOST[/PATH], got {url:?}"
        )))
    }
}

/// How much a [`BlobWriter`] holds before it sends a part. R2 and S3 take
/// parts of at least 5 MiB (all but the last), and all the same size.
pub const PART_BYTES: usize = 8 << 20;

/// An object written as it arrives ([`BlobStore::writer`]). An object
/// under [`PART_BYTES`] goes up in one put at the end; a larger one as a
/// multipart upload, a part at a time, completed at the end. It holds at
/// most one part in memory.
pub struct BlobWriter {
    store: Arc<dyn ObjectStore>,
    key: Key,
    options: PutOptions,
    buf: Vec<u8>,
    upload: Option<Box<dyn object_store::MultipartUpload>>,
    size: u64,
}

impl BlobWriter {
    /// Bytes written so far.
    pub fn size(&self) -> u64 {
        self.size
    }

    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.size += data.len() as u64;
        let mut data = data;
        while !data.is_empty() {
            let take = (PART_BYTES - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buf.len() == PART_BYTES {
                let part = std::mem::take(&mut self.buf);
                self.put_part(part).await?;
            }
        }
        Ok(())
    }

    async fn put_part(&mut self, part: Vec<u8>) -> Result<()> {
        if self.upload.is_none() {
            let opts = object_store::PutMultipartOptions {
                attributes: self.options.attributes.clone(),
                ..Default::default()
            };
            self.upload = Some(self.store.put_multipart_opts(&self.key, opts).await?);
        }
        let upload = self.upload.as_mut().expect("started");
        upload.put_part(PutPayload::from(Bytes::from(part))).await?;
        Ok(())
    }

    /// Stores what was written under the key.
    pub async fn finish(mut self) -> Result<()> {
        let rest = std::mem::take(&mut self.buf);
        let Some(mut upload) = self.upload.take() else {
            let payload = PutPayload::from(Bytes::from(rest));
            self.store
                .put_opts(&self.key, payload, self.options.clone())
                .await?;
            return Ok(());
        };
        let done = async {
            if !rest.is_empty() {
                upload.put_part(PutPayload::from(Bytes::from(rest))).await?;
            }
            upload.complete().await
        }
        .await;
        if let Err(e) = done {
            let _ = upload.abort().await;
            return Err(e.into());
        }
        Ok(())
    }

    /// Drops what was written; the key keeps what it had.
    pub async fn abort(mut self) {
        if let Some(mut upload) = self.upload.take() {
            let _ = upload.abort().await;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "h2live-blobs-{name}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ))
    }

    /// What every blob store must do; run against a directory, memory and
    /// an S3 endpoint (a fake one, tests/blobs_s3.rs).
    pub async fn conformance(store: &BlobStore) {
        let meta = Meta {
            content_type: Some("application/octet-stream".into()),
            ..Meta::default()
        };
        assert_eq!(store.get("content/ab/abcd").await.unwrap(), None);
        assert_eq!(store.head("content/ab/abcd").await.unwrap(), None);
        store.delete("content/ab/abcd").await.unwrap();

        store
            .put("content/ab/abcd", Bytes::from_static(b"hello"), &meta)
            .await
            .unwrap();
        assert_eq!(
            store.get("content/ab/abcd").await.unwrap().as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(
            store.head("content/ab/abcd").await.unwrap().unwrap().size,
            5
        );
        assert_eq!(
            store
                .get_prefix("content/ab/abcd", 4)
                .await
                .unwrap()
                .as_deref(),
            Some(&b"hell"[..])
        );
        let (size, mut stream) = store.get_stream("content/ab/abcd").await.unwrap().unwrap();
        assert_eq!(size, 5);
        let mut got = Vec::new();
        while let Some(chunk) = stream.next().await {
            got.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(got, b"hello");

        // A file in, larger than one read.
        let dir = temp_dir("put-file");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("upload");
        let big: Vec<u8> = (0..(3 << 20) + 17).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file, &big).unwrap();
        store
            .put_file("storage/42/blobs/cd/cdef", &file, &meta)
            .await
            .unwrap();
        assert_eq!(
            store
                .get("storage/42/blobs/cd/cdef")
                .await
                .unwrap()
                .as_deref(),
            Some(&big[..])
        );
        let _ = std::fs::remove_dir_all(&dir);

        // Listing is by whole prefix, in key order.
        store
            .put("content/ff/ffff", Bytes::from_static(b"x"), &meta)
            .await
            .unwrap();
        store
            .put("contents/not-me", Bytes::from_static(b"y"), &meta)
            .await
            .unwrap();
        let keys: Vec<_> = store
            .list("content/")
            .await
            .unwrap()
            .into_iter()
            .map(|i| (i.key, i.size))
            .collect();
        assert_eq!(
            keys,
            [
                ("content/ab/abcd".to_string(), 5),
                ("content/ff/ffff".to_string(), 1)
            ]
        );
        assert!(store.list("nothing/").await.unwrap().is_empty());

        // Conditional writes: create once, then only over what was seen.
        let idx = "storage/42/index.json";
        let first = store
            .put_if(idx, Bytes::from_static(b"{1}"), &Seen::default(), &meta)
            .await
            .unwrap();
        let (bytes, seen) = store.get_seen(idx).await.unwrap().unwrap();
        assert_eq!(&bytes[..], b"{1}");
        if !store.is_dir() {
            assert!(matches!(
                store
                    .put_if(idx, Bytes::from_static(b"{x}"), &Seen::default(), &meta)
                    .await,
                Err(BlobError::Changed(_))
            ));
        }
        let second = store
            .put_if(idx, Bytes::from_static(b"{2}"), &seen, &meta)
            .await
            .unwrap();
        if !store.is_dir() {
            assert_ne!(first, second);
            assert!(matches!(
                store
                    .put_if(idx, Bytes::from_static(b"{3}"), &seen, &meta)
                    .await,
                Err(BlobError::Changed(_))
            ));
        }
        assert_eq!(&store.get(idx).await.unwrap().unwrap()[..], b"{2}");

        store.delete("content/ab/abcd").await.unwrap();
        assert_eq!(store.get("content/ab/abcd").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_directory_conforms_and_keeps_the_old_layout() {
        let root = temp_dir("dir");
        let store = BlobStore::dir(&root).unwrap();
        conformance(&store).await;
        // The data directory's layout, so existing servers' files are found.
        assert_eq!(
            std::fs::read(root.join("storage/42/blobs/cd/cdef"))
                .unwrap()
                .len(),
            (3 << 20) + 17
        );
        assert!(root.join("content/ff/ffff").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A writer's object shows only when finished, whole, in one put or in
    /// parts; an abort leaves what the key had.
    async fn writes_as_it_arrives(store: &BlobStore) {
        let meta = Meta::default();
        // Small: one put at the end.
        let mut w = store.writer("content/aa/small", &meta).unwrap();
        w.write(b"hello ").await.unwrap();
        w.write(b"world").await.unwrap();
        assert_eq!(w.size(), 11);
        assert_eq!(store.get("content/aa/small").await.unwrap(), None);
        w.finish().await.unwrap();
        assert_eq!(
            store.get("content/aa/small").await.unwrap().unwrap(),
            &b"hello world"[..]
        );

        // Large: parts, in writes that do not line up with them.
        let big: Vec<u8> = (0..PART_BYTES * 2 + 12345)
            .map(|i| (i % 251) as u8)
            .collect();
        let mut w = store.writer("content/bb/big", &meta).unwrap();
        for chunk in big.chunks(1 << 20 | 7) {
            w.write(chunk).await.unwrap();
        }
        assert_eq!(store.get("content/bb/big").await.unwrap(), None);
        w.finish().await.unwrap();
        assert_eq!(
            store.get("content/bb/big").await.unwrap().unwrap(),
            &big[..]
        );

        // An abort, past a part: the key keeps what it had.
        let mut w = store.writer("content/aa/small", &meta).unwrap();
        w.write(&big[..PART_BYTES + 1]).await.unwrap();
        w.abort().await;
        assert_eq!(
            store.get("content/aa/small").await.unwrap().unwrap(),
            &b"hello world"[..]
        );
    }

    #[tokio::test]
    async fn memory_writes_as_it_arrives() {
        writes_as_it_arrives(&BlobStore::memory()).await;
    }

    #[tokio::test]
    async fn a_directory_writes_as_it_arrives() {
        let root = temp_dir("writer");
        writes_as_it_arrives(&BlobStore::dir(&root).unwrap()).await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn memory_conforms() {
        conformance(&BlobStore::memory()).await;
    }

    #[tokio::test]
    async fn a_directory_renames_uploads_into_place() {
        let root = temp_dir("rename");
        let store = BlobStore::dir(&root).unwrap();
        let tmp = root.join("tmp/upload");
        std::fs::create_dir_all(tmp.parent().unwrap()).unwrap();
        std::fs::write(&tmp, b"bytes").unwrap();
        store
            .put_file("content/aa/aabb", &tmp, &Meta::default())
            .await
            .unwrap();
        assert!(!tmp.exists(), "moved, not copied");
        assert_eq!(
            std::fs::read(root.join("content/aa/aabb")).unwrap(),
            b"bytes"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Just enough of S3 for object_store's client: path-style PUT (with
    /// If-Match / If-None-Match), GET (with Range), HEAD, DELETE and
    /// ListObjectsV2. It records each request's Authorization header so the
    /// test can see the requests are signed for region `auto`.
    mod fake_s3 {
        use std::collections::BTreeMap;
        use std::sync::{Arc, Mutex};

        use axum::Router;
        use axum::body::Bytes;
        use axum::extract::{Path, Query, State};
        use axum::http::{HeaderMap, Method, StatusCode, header};
        use axum::response::{IntoResponse, Response};

        #[derive(Default)]
        pub struct Bucket {
            pub objects: BTreeMap<String, (Bytes, u64, HeaderMap)>,
            pub auth: Vec<String>,
            next: u64,
        }

        pub type Shared = Arc<Mutex<Bucket>>;

        const MODIFIED: &str = "Mon, 01 Jan 2024 00:00:00 GMT";

        fn object_headers(etag: u64, size: u64, stored: &HeaderMap) -> HeaderMap {
            let mut h = HeaderMap::new();
            h.insert(header::ETAG, format!("\"{etag}\"").parse().unwrap());
            h.insert(header::LAST_MODIFIED, MODIFIED.parse().unwrap());
            h.insert(header::CONTENT_LENGTH, size.into());
            for name in [
                header::CONTENT_TYPE,
                header::CONTENT_ENCODING,
                header::CACHE_CONTROL,
            ] {
                if let Some(v) = stored.get(&name) {
                    h.insert(name, v.clone());
                }
            }
            h
        }

        async fn object(
            State(b): State<Shared>,
            method: Method,
            Path((_bucket, key)): Path<(String, String)>,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            let mut b = b.lock().unwrap();
            if let Some(a) = headers.get(header::AUTHORIZATION) {
                b.auth.push(a.to_str().unwrap().to_string());
            }
            let current = b.objects.get(&key).map(|o| o.1);
            match method {
                Method::PUT => {
                    let none_match = headers.get(header::IF_NONE_MATCH).is_some();
                    let if_match = headers
                        .get(header::IF_MATCH)
                        .map(|v| v.to_str().unwrap().trim_matches('"').to_string());
                    if (none_match && current.is_some())
                        || if_match.is_some_and(|m| Some(m) != current.map(|c| c.to_string()))
                    {
                        return StatusCode::PRECONDITION_FAILED.into_response();
                    }
                    b.next += 1;
                    let etag = b.next;
                    b.objects.insert(key, (body, etag, headers));
                    let mut h = HeaderMap::new();
                    h.insert(header::ETAG, format!("\"{etag}\"").parse().unwrap());
                    (StatusCode::OK, h).into_response()
                }
                Method::DELETE => {
                    b.objects.remove(&key);
                    StatusCode::NO_CONTENT.into_response()
                }
                Method::GET | Method::HEAD => {
                    let Some((bytes, etag, stored)) = b.objects.get(&key).cloned() else {
                        return StatusCode::NOT_FOUND.into_response();
                    };
                    let size = bytes.len() as u64;
                    let mut h = object_headers(etag, size, &stored);
                    let range = headers.get(header::RANGE).and_then(|r| {
                        r.to_str()
                            .ok()?
                            .strip_prefix("bytes=")?
                            .split_once('-')
                            .map(|(a, z)| (a.parse::<u64>().ok(), z.parse::<u64>().ok()))
                    });
                    if let Some((Some(start), end)) = range {
                        let end = end.unwrap_or(size - 1).min(size - 1);
                        let part = bytes.slice(start as usize..=end as usize);
                        h.insert(header::CONTENT_LENGTH, (part.len() as u64).into());
                        h.insert(
                            header::CONTENT_RANGE,
                            format!("bytes {start}-{end}/{size}").parse().unwrap(),
                        );
                        let body = if method == Method::HEAD {
                            Bytes::new()
                        } else {
                            part
                        };
                        return (StatusCode::PARTIAL_CONTENT, h, body).into_response();
                    }
                    let body = if method == Method::HEAD {
                        Bytes::new()
                    } else {
                        bytes
                    };
                    (StatusCode::OK, h, body).into_response()
                }
                _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
            }
        }

        async fn list(
            State(b): State<Shared>,
            Path(bucket): Path<String>,
            Query(q): Query<BTreeMap<String, String>>,
        ) -> Response {
            let b = b.lock().unwrap();
            let prefix = q.get("prefix").cloned().unwrap_or_default();
            let mut xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{bucket}</Name><Prefix>{prefix}</Prefix><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>"
            );
            for (key, (bytes, etag, _)) in b.objects.range(prefix.clone()..) {
                if !key.starts_with(&prefix) {
                    break;
                }
                xml.push_str(&format!(
                    "<Contents><Key>{key}</Key><LastModified>2024-01-01T00:00:00.000Z</LastModified><ETag>&quot;{etag}&quot;</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",
                    bytes.len()
                ));
            }
            xml.push_str("</ListBucketResult>");
            ([(header::CONTENT_TYPE, "application/xml")], xml).into_response()
        }

        /// DeleteObjects (`POST /{bucket}?delete`), which object_store uses
        /// for every delete.
        async fn delete_many(State(b): State<Shared>, body: String) -> Response {
            let mut b = b.lock().unwrap();
            let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><DeleteResult>");
            for part in body.split("<Key>").skip(1) {
                let key = part.split("</Key>").next().unwrap_or("");
                b.objects.remove(key);
                out.push_str(&format!("<Deleted><Key>{key}</Key></Deleted>"));
            }
            out.push_str("</DeleteResult>");
            ([(header::CONTENT_TYPE, "application/xml")], out).into_response()
        }

        /// Serves on a free loopback port; returns its URL.
        pub async fn start() -> (String, Shared) {
            let shared = Shared::default();
            let app = Router::new()
                .route("/{bucket}", axum::routing::get(list).post(delete_many))
                .route("/{bucket}/{*key}", axum::routing::any(object))
                .layer(axum::extract::DefaultBodyLimit::max(64 << 20))
                .with_state(shared.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            (url, shared)
        }
    }

    /// The S3 backend against the fake: the conformance suite, requests
    /// signed for region `auto` with the configured key, and a prefix
    /// keeping one server's keys apart.
    #[tokio::test]
    async fn s3_conforms_against_a_fake_endpoint() {
        let (endpoint, bucket) = fake_s3::start().await;
        let vars = [
            (ENV_ENDPOINT.to_string(), endpoint),
            (ENV_ACCESS_KEY.to_string(), "AKTEST".to_string()),
            (ENV_SECRET_KEY.to_string(), "not-a-secret".to_string()),
        ];
        let env = move |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let store = BlobStore::open(&Location::parse("s3://h2live").unwrap(), &env, false).unwrap();
        assert_eq!(store.describe(), "s3://h2live");
        conformance(&store).await;
        {
            let b = bucket.lock().unwrap();
            assert!(!b.auth.is_empty());
            for a in &b.auth {
                assert!(
                    a.starts_with("AWS4-HMAC-SHA256 Credential=AKTEST/") && a.contains("/auto/s3/"),
                    "{a}"
                );
            }
            let stored = &b.objects["storage/42/blobs/cd/cdef"].2;
            assert_eq!(
                stored[axum::http::header::CONTENT_TYPE],
                "application/octet-stream"
            );
        }
        let prefixed =
            BlobStore::open(&Location::parse("s3://h2live/one").unwrap(), &env, false).unwrap();
        prefixed
            .put("content/aa/aa", Bytes::from_static(b"p"), &Meta::default())
            .await
            .unwrap();
        assert!(
            bucket
                .lock()
                .unwrap()
                .objects
                .contains_key("one/content/aa/aa")
        );
        assert_eq!(prefixed.list("content/").await.unwrap().len(), 1);
    }

    #[test]
    fn locations_and_public_urls() {
        assert_eq!(
            Location::parse("s3://h2live").unwrap(),
            Location::S3 {
                bucket: "h2live".into(),
                prefix: String::new()
            }
        );
        assert_eq!(
            Location::parse("s3://h2live/live/").unwrap(),
            Location::S3 {
                bucket: "h2live".into(),
                prefix: "live".into()
            }
        );
        assert_eq!(
            Location::parse("/srv/data").unwrap(),
            Location::Dir("/srv/data".into())
        );
        assert_eq!(
            Location::parse("file:/a b/data").unwrap(),
            Location::Dir("/a b/data".into())
        );
        assert_eq!(Location::parse("memory:").unwrap(), Location::Memory);
        assert!(Location::parse("s3://").is_err());
        assert!(Location::parse("gs://bucket").is_err());

        let public = BlobStore::memory().with_public_url("https://art.example.net/");
        assert_eq!(
            public.public_url("art/0123/map/x y/map.glb").as_deref(),
            Some("https://art.example.net/art/0123/map/x%20y/map.glb")
        );
        assert_eq!(BlobStore::memory().public_url("art/x"), None);
    }

    #[test]
    fn s3_settings_come_from_the_environment() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let all = env(&[
            (ENV_ENDPOINT, "https://acct.r2.cloudflarestorage.com"),
            (ENV_ACCESS_KEY, "id"),
            (ENV_SECRET_KEY, "secret"),
            ("H2_LIVE_S3_PUBLIC_ACCESS_KEY_ID", "public-id"),
        ]);
        let c = S3Config::from_env(&all, false).unwrap();
        assert_eq!(c.region, "auto");
        assert_eq!(c.access_key_id, "id");
        assert!(
            !format!("{c:?}").contains("secret"),
            "secrets stay out of logs"
        );
        let p = S3Config::from_env(&all, true).unwrap();
        assert_eq!(p.access_key_id, "public-id");
        assert_eq!(p.secret_access_key, "secret", "falls back");
        let none = env(&[(ENV_ENDPOINT, "https://x")]);
        let err = S3Config::from_env(&none, false).unwrap_err().to_string();
        assert!(err.contains(ENV_ACCESS_KEY), "{err}");
    }

    #[test]
    fn a_public_store_needs_its_url() {
        let data = temp_dir("cfg");
        let none = |_: &str| None;
        let b = Blobs::configure(&data, None, None, None, &none).unwrap();
        assert!(b.private.is_dir() && b.public.is_none());
        assert!(Blobs::configure(&data, None, Some("memory:"), None, &none).is_err());
        assert!(Blobs::configure(&data, None, None, Some("https://x"), &none).is_err());
        assert!(
            Blobs::configure(&data, None, Some("memory:"), Some("art.example"), &none).is_err()
        );
        let b = Blobs::configure(
            &data,
            Some("memory:"),
            Some("memory:"),
            Some("https://x"),
            &none,
        )
        .unwrap();
        assert_eq!(
            b.public.unwrap().public_url("a").as_deref(),
            Some("https://x/a")
        );
        assert!(
            Blobs::configure(&data, Some("s3://b"), None, None, &none).is_err(),
            "no keys"
        );
        let _ = std::fs::remove_dir_all(&data);
    }
}
