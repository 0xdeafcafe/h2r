//! The blob store against a real S3 protocol server, which is what
//! Cloudflare R2 is, rather than the hand-written fake in `blobs.rs`:
//! `s3s` (an S3 server library) over a directory, in process, signing and
//! all. Every byte is invented.
//!
//! The same tests run against any other S3-compatible endpoint (MinIO,
//! Ceph, a scratch R2 bucket) when `H2_S3_TEST_ENDPOINT`,
//! `H2_S3_TEST_ACCESS_KEY`, `H2_S3_TEST_SECRET_KEY` and `H2_S3_TEST_BUCKET`
//! (an existing bucket) are set; each test keeps to its own random key
//! prefix and removes what it wrote. `H2_S3_TEST_REGION` overrides `auto`.
mod common;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use bytes::Bytes;
use common::*;
use content::Sha256;
use futures_util::StreamExt;
use live_proto::lxm;
use live_server::Policy;
use live_server::art::{GameArt, PUBLISHED_CACHE, PublishedArt};
use live_server::blobs::{BlobError, BlobStore, Blobs, Meta, S3Config, Seen};
use live_server::cloud_saves::StorageLimits;
use live_server::content::ContentLimits;
use live_server::store::MemoryStore;
use object_store::{Attribute, ObjectStoreExt};

const KEY: &str = "AKIATESTTESTTEST";
const SECRET: &str = "test-secret-test-secret-test-secret";

/// A bucket to test in: the in-process server, or the one from the
/// environment.
struct Bucket {
    config: S3Config,
    bucket: String,
    /// This test's key prefix: `s3://bucket/prefix`.
    prefix: String,
    _scratch: Option<PathBuf>,
}

impl Bucket {
    async fn start() -> Self {
        let prefix = format!("h2-s3-test-{}", live_proto::sign::random_id());
        let env = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
        if let Some(endpoint) = env("H2_S3_TEST_ENDPOINT") {
            let need = |n: &str| env(n).unwrap_or_else(|| panic!("{n} is not set"));
            return Bucket {
                config: S3Config {
                    endpoint,
                    access_key_id: need("H2_S3_TEST_ACCESS_KEY"),
                    secret_access_key: need("H2_S3_TEST_SECRET_KEY"),
                    region: env("H2_S3_TEST_REGION").unwrap_or_else(|| "auto".into()),
                },
                bucket: need("H2_S3_TEST_BUCKET"),
                prefix,
                _scratch: None,
            };
        }
        let root = std::env::temp_dir().join(format!("h2live-s3-{prefix}"));
        std::fs::create_dir_all(root.join("h2live")).unwrap();
        let fs = s3s_fs::FileSystem::new(&root).unwrap();
        let mut builder = s3s::service::S3ServiceBuilder::new(fs);
        builder.set_auth(s3s::auth::SimpleAuth::from_single(KEY, SECRET));
        let service = builder.build();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use hyper_util::rt::{TokioExecutor, TokioIo};
            let http = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let conn = http
                    .serve_connection(TokioIo::new(socket), service.clone())
                    .into_owned();
                tokio::spawn(async move {
                    let _ = conn.await;
                });
            }
        });
        Bucket {
            config: S3Config {
                endpoint: format!("http://{addr}"),
                access_key_id: KEY.into(),
                secret_access_key: SECRET.into(),
                region: "auto".into(),
            },
            bucket: "h2live".into(),
            prefix,
            _scratch: Some(root),
        }
    }

    fn store(&self) -> BlobStore {
        BlobStore::s3(&self.bucket, &self.prefix, &self.config).unwrap()
    }

    fn store_at(&self, sub: &str) -> BlobStore {
        BlobStore::s3(
            &self.bucket,
            &format!("{}/{sub}", self.prefix),
            &self.config,
        )
        .unwrap()
    }

    /// The raw client, for what [`BlobStore`] does not expose (headers,
    /// ranges from the middle).
    fn raw(&self) -> object_store::aws::AmazonS3 {
        object_store::aws::AmazonS3Builder::new()
            .with_bucket_name(&self.bucket)
            .with_endpoint(&self.config.endpoint)
            .with_region(&self.config.region)
            .with_access_key_id(&self.config.access_key_id)
            .with_secret_access_key(&self.config.secret_access_key)
            .with_allow_http(self.config.endpoint.starts_with("http://"))
            .with_virtual_hosted_style_request(false)
            .build()
            .unwrap()
    }

    fn raw_key(&self, key: &str) -> object_store::path::Path {
        object_store::path::Path::from(format!("{}/{key}", self.prefix))
    }

    fn env(&self) -> impl Fn(&str) -> Option<String> + use<> {
        let c = self.config.clone();
        move |n| match n {
            "H2_LIVE_S3_ENDPOINT" => Some(c.endpoint.clone()),
            "H2_LIVE_S3_ACCESS_KEY_ID" => Some(c.access_key_id.clone()),
            "H2_LIVE_S3_SECRET_ACCESS_KEY" => Some(c.secret_access_key.clone()),
            "H2_LIVE_S3_REGION" => Some(c.region.clone()),
            _ => None,
        }
    }

    /// Everything this test wrote.
    async fn clean(&self) {
        let store = self.store();
        for i in store.list("").await.unwrap() {
            store.delete(&i.key).await.unwrap();
        }
    }
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

fn octets() -> Meta {
    Meta {
        content_type: Some("application/octet-stream".into()),
        ..Meta::default()
    }
}

#[tokio::test]
async fn put_get_head_range_list_delete() {
    let b = Bucket::start().await;
    let store = b.store();
    let meta = octets();
    assert_eq!(store.get("content/ab/abcd").await.unwrap(), None);
    assert_eq!(store.head("content/ab/abcd").await.unwrap(), None);
    store.delete("content/ab/abcd").await.unwrap();

    store
        .put("content/ab/abcd", Bytes::from_static(b"hello world"), &meta)
        .await
        .unwrap();
    assert_eq!(
        store.get("content/ab/abcd").await.unwrap().as_deref(),
        Some(&b"hello world"[..])
    );
    assert_eq!(
        store.head("content/ab/abcd").await.unwrap().unwrap().size,
        11
    );
    assert_eq!(
        store
            .get_prefix("content/ab/abcd", 5)
            .await
            .unwrap()
            .as_deref(),
        Some(&b"hello"[..])
    );
    let (size, mut stream) = store.get_stream("content/ab/abcd").await.unwrap().unwrap();
    assert_eq!(size, 11);
    let mut got = Vec::new();
    while let Some(chunk) = stream.next().await {
        got.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(got, b"hello world");

    // A range from the middle, through the raw client (BlobStore reads
    // prefixes only).
    let mid = b
        .raw()
        .get_range(&b.raw_key("content/ab/abcd"), 6..11)
        .await
        .unwrap();
    assert_eq!(&mid[..], b"world");

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
            ("content/ab/abcd".to_string(), 11),
            ("content/ff/ffff".to_string(), 1)
        ]
    );
    assert!(store.list("nothing/").await.unwrap().is_empty());

    store.delete("content/ab/abcd").await.unwrap();
    assert_eq!(store.get("content/ab/abcd").await.unwrap(), None);
    b.clean().await;
    assert!(store.list("").await.unwrap().is_empty());
}

#[tokio::test]
async fn a_large_file_goes_up_in_parts_and_comes_back_whole() {
    let b = Bucket::start().await;
    let store = b.store();
    let dir = std::env::temp_dir().join(format!("h2live-s3-big-{}", b.prefix));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("upload");
    // Over two part sizes, and not a multiple of one.
    let big = pattern(25 * (1 << 20) + 123);
    std::fs::write(&file, &big).unwrap();
    store
        .put_file("content/aa/big", &file, &octets())
        .await
        .unwrap();
    assert_eq!(
        store.head("content/aa/big").await.unwrap().unwrap().size,
        big.len() as u64
    );
    let got = store.get("content/aa/big").await.unwrap().unwrap();
    assert_eq!(got.len(), big.len());
    assert!(got[..] == big[..], "the bytes differ");
    assert_eq!(
        store
            .get_prefix("content/aa/big", 1000)
            .await
            .unwrap()
            .as_deref(),
        Some(&big[..1000])
    );
    // A range read across a part boundary.
    let edge = 10usize << 20;
    let r = b
        .raw()
        .get_range(
            &b.raw_key("content/aa/big"),
            (edge - 5) as u64..(edge + 5) as u64,
        )
        .await
        .unwrap();
    assert_eq!(&r[..], &big[edge - 5..edge + 5]);
    let _ = std::fs::remove_dir_all(&dir);
    b.clean().await;
}

#[tokio::test]
async fn conditional_writes_refuse_a_changed_object() {
    let b = Bucket::start().await;
    let store = b.store();
    let meta = Meta {
        content_type: Some("application/json".into()),
        ..Meta::default()
    };
    let idx = "storage/42/index.json";
    store
        .put_if(idx, Bytes::from_static(b"{1}"), &Seen::default(), &meta)
        .await
        .unwrap();
    let (bytes, seen) = store.get_seen(idx).await.unwrap().unwrap();
    assert_eq!(&bytes[..], b"{1}");
    assert!(
        matches!(
            store
                .put_if(idx, Bytes::from_static(b"{x}"), &Seen::default(), &meta)
                .await,
            Err(BlobError::Changed(_))
        ),
        "create over an existing object"
    );
    store
        .put_if(idx, Bytes::from_static(b"{2}"), &seen, &meta)
        .await
        .unwrap();
    assert!(
        matches!(
            store
                .put_if(idx, Bytes::from_static(b"{3}"), &seen, &meta)
                .await,
            Err(BlobError::Changed(_))
        ),
        "update over a write this one did not see"
    );
    assert_eq!(store.get(idx).await.unwrap().as_deref(), Some(&b"{2}"[..]));
    b.clean().await;
}

#[tokio::test]
async fn content_type_encoding_and_cache_control_are_kept() {
    let b = Bucket::start().await;
    let store = b.store();
    store
        .put(
            "art/x/a.js",
            Bytes::from_static(b"gz-bytes"),
            &Meta {
                content_type: Some("text/javascript".into()),
                content_encoding: Some("gzip".into()),
                cache_control: Some(PUBLISHED_CACHE.into()),
            },
        )
        .await
        .unwrap();
    let got = b.raw().get(&b.raw_key("art/x/a.js")).await.unwrap();
    let attr = |a: Attribute| got.attributes.get(&a).map(|v| v.as_ref().to_string());
    assert_eq!(
        attr(Attribute::ContentType).as_deref(),
        Some("text/javascript")
    );
    assert_eq!(attr(Attribute::ContentEncoding).as_deref(), Some("gzip"));
    assert_eq!(
        attr(Attribute::CacheControl).as_deref(),
        Some(PUBLISHED_CACHE)
    );
    b.clean().await;
}

#[tokio::test]
async fn wrong_credentials_and_a_missing_bucket_are_errors_not_empty() {
    let b = Bucket::start().await;
    let mut bad = b.config.clone();
    bad.secret_access_key = "not-the-secret".into();
    let store = BlobStore::s3(&b.bucket, &b.prefix, &bad).unwrap();
    assert!(store.get("content/aa/x").await.is_err());
    assert!(store.list("").await.is_err());
    let nobucket = BlobStore::s3("no-such-bucket-h2", &b.prefix, &b.config).unwrap();
    assert!(nobucket.list("").await.is_err());
}

fn big_limits() -> Policy {
    Policy {
        content: ContentLimits {
            layer_bytes: 64 << 20,
            map_bytes: 64 << 20,
            quota_bytes: 1 << 30,
            upload_rate: (1000.0, 1000),
        },
        storage: StorageLimits::default(),
        write_rate: (1000.0, 1000),
        ..Policy::default()
    }
}

/// The server's own paths over the bucket: a content upload (small, then
/// large enough for a multipart upload), a download, a cloud save with its
/// index, and published art with its headers.
#[tokio::test]
async fn the_server_serves_content_saves_and_art_from_a_bucket() {
    let b = Bucket::start().await;
    let h = start(Arc::new(MemoryStore::new()), big_limits()).await;
    let private = b.store_at("private");
    let public = b
        .store_at("public")
        .with_public_url("https://art.example.test");
    h.live.set_blobs(Blobs {
        private: private.clone(),
        public: Some(public.clone()),
    });
    h.live.set_data_dir(&h.dir);
    let chief = h.create("Chief").await;

    // Custom content, small and large.
    for (label, bytes) in [("small", pattern(5000)), ("large", pattern(12 << 20))] {
        let hash = Sha256::of(&bytes);
        let r = h
            .call(
                Method::PUT,
                &format!("/v1/content/{hash}"),
                &[h.bearer(&chief, lxm::CONTENT_WRITE)],
                Body::from(bytes.clone()),
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{label}: {}", r.error());
        let hex = hash.to_string();
        let info = private
            .head(&format!("content/{}/{hex}", &hex[..2]))
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{label}: not in the bucket"));
        assert_eq!(info.size, bytes.len() as u64);
        assert!(!h.dir.join("content").join(&hex[..2]).join(&hex).exists());
        let r = h.get(&format!("/v1/content/{hash}")).await;
        assert_eq!(r.status, StatusCode::OK, "{label}");
        assert!(r.body == bytes, "{label}: the download differs");
    }

    // A cloud save, replaced, then deleted.
    let item = format!(
        "/v1/accounts/{0}/storage/save/{1}",
        chief.xuid,
        live_proto::storage::encode_name("a")
    );
    let save = |if_match: &'static str, bytes: &'static [u8]| {
        let headers = vec![
            h.bearer(&chief, lxm::STORAGE_WRITE),
            ("if-match", if_match.to_string()),
            ("x-h2-sha256", Sha256::of(bytes).to_string()),
            ("x-h2-modified", "1700000000000".to_string()),
        ];
        let (h, item) = (&h, item.clone());
        async move {
            h.call(Method::PUT, &item, &headers, Body::from(bytes.to_vec()))
                .await
        }
    };
    assert_eq!(save("0", b"first").await.status, StatusCode::CREATED);
    let r = save("1", b"second").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let got = h
        .call(
            Method::GET,
            &item,
            &[h.bearer(&chief, lxm::STORAGE_READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(got.body, b"second");
    let prefix = format!("storage/{}/", chief.xuid);
    let hex = Sha256::of(b"second").to_string();
    let keys: Vec<_> = private
        .list(&prefix)
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.key)
        .collect();
    assert_eq!(
        keys,
        [
            format!("{prefix}blobs/{}/{hex}", &hex[..2]),
            format!("{prefix}index.json")
        ]
    );
    let r = h
        .call(
            Method::DELETE,
            &item,
            &[
                h.bearer(&chief, lxm::STORAGE_WRITE),
                ("if-match", "2".into()),
            ],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    assert!(
        private
            .list(&format!("{prefix}blobs/"))
            .await
            .unwrap()
            .is_empty()
    );

    // Art: published once, with its headers, and again for nothing.
    let art_dir = h.dir.join("art-src");
    std::fs::create_dir_all(art_dir.join("ui")).unwrap();
    std::fs::write(art_dir.join("manifest.json"), b"{}").unwrap();
    std::fs::write(art_dir.join("ui/a.png"), b"\x89PNG not really").unwrap();
    std::fs::write(art_dir.join("ui/app.js"), b"plain").unwrap();
    std::fs::write(art_dir.join("ui/app.js.gz"), b"gzipped-bytes").unwrap();
    // A packed glb (art_pack): its Brotli copy goes up, not the gzip one.
    std::fs::write(art_dir.join("ui/m.glb"), b"glTF").unwrap();
    std::fs::write(art_dir.join("ui/m.glb.gz"), b"gz-glb").unwrap();
    std::fs::write(art_dir.join("ui/m.glb.br"), b"br-glb").unwrap();
    let art = GameArt {
        dir: art_dir,
        share: true,
    };
    let published = PublishedArt::new(public.clone());
    let report = published.publish(&art).await.unwrap();
    assert_eq!((report.files, report.uploaded), (4, 4));
    let again = published.publish(&art).await.unwrap();
    assert_eq!(again.uploaded, 0);
    let base = format!("public/art/{}", report.stamp);
    let js = b
        .raw()
        .get(&b.raw_key(&format!("{base}/ui/app.js")))
        .await
        .unwrap();
    let a = |r: &object_store::GetResult, at: Attribute| {
        r.attributes.get(&at).map(|v| v.as_ref().to_string())
    };
    assert_eq!(a(&js, Attribute::ContentEncoding).as_deref(), Some("gzip"));
    assert_eq!(
        a(&js, Attribute::CacheControl).as_deref(),
        Some(PUBLISHED_CACHE)
    );
    assert!(
        a(&js, Attribute::ContentType)
            .unwrap()
            .contains("javascript")
    );
    assert_eq!(&js.bytes().await.unwrap()[..], b"gzipped-bytes");
    let png = b
        .raw()
        .get(&b.raw_key(&format!("{base}/ui/a.png")))
        .await
        .unwrap();
    assert_eq!(
        a(&png, Attribute::ContentType).as_deref(),
        Some("image/png")
    );
    assert_eq!(a(&png, Attribute::ContentEncoding), None);
    let glb = b
        .raw()
        .get(&b.raw_key(&format!("{base}/ui/m.glb")))
        .await
        .unwrap();
    assert_eq!(a(&glb, Attribute::ContentEncoding).as_deref(), Some("br"));
    assert_eq!(
        a(&glb, Attribute::ContentType).as_deref(),
        Some("model/gltf-binary")
    );
    assert_eq!(
        a(&glb, Attribute::CacheControl).as_deref(),
        Some(PUBLISHED_CACHE)
    );
    assert_eq!(&glb.bytes().await.unwrap()[..], b"br-glb");

    b.clean().await;
}

/// `store copy` from a data directory to a bucket, twice (the second
/// carries on), and back out; the bytes match.
#[tokio::test]
async fn store_copy_moves_blobs_to_a_bucket_and_resumes() {
    let b = Bucket::start().await;
    let dir = std::env::temp_dir().join(format!("h2live-s3-copy-{}", b.prefix));
    let files = [
        ("content/ab/abcd", pattern(3000)),
        ("content/cd/cdef", pattern(11 << 20)),
        ("storage/7/blobs/ee/eeff", pattern(70_000)),
        ("storage/7/index.json", b"{\"items\":[]}".to_vec()),
    ];
    for (k, bytes) in &files {
        let p = dir.join(k);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    // Not blobs: the key stays behind.
    std::fs::write(dir.join("server.key"), b"secret").unwrap();
    let env = b.env();
    let to = format!("s3://{}/{}/copied", b.bucket, b.prefix);
    let up = || {
        vec![
            "--from-blobs".to_string(),
            dir.display().to_string(),
            "--to-blobs".to_string(),
            to.clone(),
        ]
    };
    live_server::copy::run(up(), &env).await.unwrap();
    let dest = b.store_at("copied");
    let listed: Vec<_> = dest
        .list("")
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.key)
        .collect();
    assert_eq!(listed, files.iter().map(|f| f.0).collect::<Vec<_>>());
    for (k, bytes) in &files {
        let got = dest.get(k).await.unwrap().unwrap();
        assert!(got[..] == bytes[..], "{k} differs");
    }
    // Again: a new file is picked up, the rest skipped.
    std::fs::create_dir_all(dir.join("content/ff")).unwrap();
    std::fs::write(dir.join("content/ff/ffff"), b"late").unwrap();
    live_server::copy::run(up(), &env).await.unwrap();
    assert_eq!(dest.list("content/").await.unwrap().len(), 3);
    // And back out into a directory.
    let back = dir.join("back");
    live_server::copy::run(
        vec![
            "--from-blobs".into(),
            to.clone(),
            "--to-blobs".into(),
            back.display().to_string(),
        ],
        &env,
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(back.join("content/cd/cdef")).unwrap(),
        files[1].1
    );
    let _ = std::fs::remove_dir_all(&dir);
    b.clean().await;
}

/// `store check`, as the user runs it with their R2 token: every step
/// passes on a working bucket and leaves nothing; a wrong secret fails the
/// steps without printing it; a public URL that is not public is a failed
/// step, and the scratch objects are removed regardless.
#[tokio::test]
async fn store_check_reports_each_step() {
    let b = Bucket::start().await;
    let env = b.env();
    let target = format!("s3://{}/{}", b.bucket, b.prefix);
    let mut lines = Vec::new();
    live_server::check::run_with(vec!["--blobs".into(), target.clone()], &env, &mut |l| {
        lines.push(l)
    })
    .await
    .unwrap_or_else(|e| panic!("{e}\n{}", lines.join("\n")));
    let out = lines.join("\n");
    assert!(out.contains("[ ok ]  multipart upload"), "{out}");
    assert!(out.contains("[ ok ]  conditional write"), "{out}");
    assert!(!out.contains("FAIL"), "{out}");
    assert!(!out.contains(&b.config.secret_access_key), "{out}");
    assert!(
        b.store().list(".h2-store-check/").await.unwrap().is_empty(),
        "left nothing behind"
    );

    // A bucket that is not public: the unauthenticated fetch is refused.
    let mut lines = Vec::new();
    let url = format!("{}/{}/{}", b.config.endpoint, b.bucket, b.prefix);
    let err = live_server::check::run_with(
        vec![
            "--blobs".into(),
            target.clone(),
            "--public-url".into(),
            url,
            "--quick".into(),
        ],
        &env,
        &mut |l| lines.push(l),
    )
    .await
    .unwrap_err();
    let out = lines.join("\n");
    assert!(err.to_string().contains("1 of"), "{err}");
    assert!(out.contains("[FAIL]  public URL"), "{out}");
    assert!(out.contains("[ ok ]  delete"), "{out}");
    assert!(b.store().list(".h2-store-check/").await.unwrap().is_empty());

    // A wrong secret: failures, and the secret appears nowhere.
    let bad_env = |n: &str| {
        if n == "H2_LIVE_S3_SECRET_ACCESS_KEY" {
            Some("wrong-secret-value".to_string())
        } else {
            env(n)
        }
    };
    let mut lines = Vec::new();
    let err = live_server::check::run_with(
        vec!["--blobs".into(), target, "--quick".into()],
        &bad_env,
        &mut |l| lines.push(l),
    )
    .await
    .unwrap_err();
    let out = lines.join("\n") + &err.to_string();
    assert!(out.contains("[FAIL]  write"), "{out}");
    assert!(!out.contains("wrong-secret-value"), "{out}");
    assert!(!out.contains(KEY), "{out}");
}
