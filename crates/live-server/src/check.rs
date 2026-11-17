//! `h2 live store check` (and `h2-live-server store check`): tries a blob
//! store with the operations the server uses, in a scratch prefix, and says
//! pass or fail for each (docs/live/live-hosting.md, "Check the bucket").
//! Meant to be run once with real Cloudflare R2 credentials before pointing
//! a server at a bucket.
//!
//! Everything it writes lives under `.h2-store-check/<random>/` in the
//! store and is deleted again, also when a step fails. Keys and secrets are
//! never printed: error text is scrubbed of them, and only the endpoint,
//! region and bucket are shown.

use anyhow::{Result, bail};
use bytes::Bytes;
use sha2::{Digest, Sha256};

use crate::blobs::{BlobError, BlobStore, Location, Meta, S3Config, Seen};

pub const USAGE: &str =
    "usage: store check --blobs s3://BUCKET[/PREFIX] [--public] [--public-url URL] [--quick]

  --blobs LOC             the store to try: s3://BUCKET[/PREFIX] (endpoint and
                          keys from $H2_LIVE_S3_ENDPOINT, $H2_LIVE_S3_ACCESS_KEY_ID,
                          $H2_LIVE_S3_SECRET_ACCESS_KEY, region $H2_LIVE_S3_REGION,
                          default auto), or a directory; else $H2_LIVE_BLOBS
  --public                try it as the public store: the $H2_LIVE_S3_PUBLIC_*
                          settings first, then the plain ones
  --public-url URL        also fetch the test object, without credentials,
                          from URL/<key> (the bucket's custom domain or
                          r2.dev address)
  --quick                 skip the 12 MiB multipart upload

Writes, reads, lists, range-reads, conditionally rewrites and deletes a few
small objects under .h2-store-check/ and prints pass or fail for each. Exits
1 if any step failed.";

/// A multipart upload needs more than one part (the writer's are 10 MiB).
const BIG: usize = 12 << 20;

/// A step's result: what it saw, or why it failed.
type Outcome = std::result::Result<String, String>;

/// Why something failed, with a hint when the cause is a common one.
fn explain(e: &BlobError) -> String {
    let text = e.to_string();
    let lower = text.to_ascii_lowercase();
    let hint = if lower.contains("403")
        || lower.contains("forbidden")
        || lower.contains("accessdenied")
        || lower.contains("access denied")
    {
        " (refused: the token lacks permission for this bucket or operation; it needs \
         Object Read & Write on this bucket)"
    } else if lower.contains("signaturedoesnotmatch") || lower.contains("invalidaccesskeyid") {
        " (the access key id or secret is wrong, or the endpoint is another account's)"
    } else if lower.contains("nosuchbucket") || lower.contains("404") {
        " (no such bucket at this endpoint: check the bucket name and the account id in \
         $H2_LIVE_S3_ENDPOINT)"
    } else if lower.contains("dns")
        || lower.contains("connect")
        || lower.contains("error sending request")
    {
        " (could not reach the endpoint: check $H2_LIVE_S3_ENDPOINT and the network)"
    } else {
        ""
    };
    format!("{text}{hint}")
}

fn scrub(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.len() >= 4) {
        out = out.replace(s.as_str(), "***");
    }
    out
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

fn sha(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

/// What to try.
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub public_url: Option<String>,
    pub quick: bool,
}

/// Runs the steps against `store`, calling `say(step, outcome)` as each
/// finishes; true if all passed. `secrets` are scrubbed from messages.
pub async fn run_steps(
    store: &BlobStore,
    options: &Options,
    secrets: &[String],
    mut say: impl FnMut(&str, Outcome),
) -> bool {
    let id = live_proto::sign::random_id();
    let base = format!(".h2-store-check/{id}");
    let small = format!("{base}/small.txt");
    let index = format!("{base}/index.json");
    let big = format!("{base}/big.bin");
    let mut all_ok = true;
    let mut step = |name: &str, outcome: Result<String, BlobError>| {
        let outcome = outcome.map_err(|e| scrub(&explain(&e), secrets));
        all_ok &= outcome.is_ok();
        say(name, outcome);
    };
    let body: &'static [u8] = b"h2 live store check: this object can be deleted";
    let meta = Meta {
        content_type: Some("text/plain".into()),
        cache_control: Some("public, max-age=60".into()),
        ..Meta::default()
    };

    step(
        "write",
        store
            .put(&small, Bytes::from_static(body), &meta)
            .await
            .map(|()| format!("{} bytes", body.len())),
    );
    step(
        "read",
        async {
            match store.get(&small).await? {
                Some(b) if b == body => Ok(format!("{} bytes, identical", b.len())),
                Some(b) => Err(BlobError::Config(format!(
                    "read back {} bytes that differ from what was written",
                    b.len()
                ))),
                None => Err(BlobError::Config(
                    "the object just written is not there".into(),
                )),
            }
        }
        .await,
    );
    step(
        "head and content type",
        async {
            let info = store
                .head(&small)
                .await?
                .ok_or_else(|| BlobError::Config("no object to look at".into()))?;
            if info.size != body.len() as u64 {
                return Err(BlobError::Config(format!(
                    "size {} not {}",
                    info.size,
                    body.len()
                )));
            }
            if store.is_dir() {
                return Ok(format!(
                    "size {} (a directory keeps no metadata)",
                    info.size
                ));
            }
            let got = store.meta(&small).await?.unwrap_or_default();
            if got.content_type != meta.content_type || got.cache_control != meta.cache_control {
                return Err(BlobError::Config(format!(
                    "the store kept content type {:?} and cache control {:?}, not what was sent",
                    got.content_type, got.cache_control
                )));
            }
            Ok(format!(
                "size {}, content type and cache control kept",
                info.size
            ))
        }
        .await,
    );
    step(
        "list",
        async {
            let listed = store.list(&format!("{base}/")).await?;
            if listed.iter().any(|i| i.key == small) {
                Ok(format!(
                    "{} object(s) under the scratch prefix",
                    listed.len()
                ))
            } else {
                Err(BlobError::Config(format!(
                    "the written object is not in the listing of its prefix ({} found)",
                    listed.len()
                )))
            }
        }
        .await,
    );
    step(
        "range read",
        async {
            match store.get_prefix(&small, 8).await? {
                Some(b) if b[..] == body[..8] => Ok("the first 8 bytes".to_string()),
                Some(b) => Err(BlobError::Config(format!(
                    "asked for 8 bytes, got {} that differ",
                    b.len()
                ))),
                None => Err(BlobError::Config("no object to read".into())),
            }
        }
        .await,
    );
    if store.is_dir() {
        step(
            "conditional write",
            Ok("skipped: a directory has one writer".into()),
        );
    } else {
        step(
            "conditional write",
            async {
                let json = Meta {
                    content_type: Some("application/json".into()),
                    ..Meta::default()
                };
                store
                    .put_if(&index, Bytes::from_static(b"{1}"), &Seen::default(), &json)
                    .await?;
                let (_, seen) = store
                    .get_seen(&index)
                    .await?
                    .ok_or_else(|| BlobError::Config("the index just created is gone".into()))?;
                // Create-only over an existing object must be refused, as
                // must an update over a write this caller did not see.
                if !matches!(
                    store
                        .put_if(&index, Bytes::from_static(b"{x}"), &Seen::default(), &json)
                        .await,
                    Err(BlobError::Changed(_))
                ) {
                    return Err(BlobError::Config(
                        "create-only (If-None-Match) overwrote an existing object: the store \
                         ignores it, and two servers could undo each other's cloud save indexes"
                            .into(),
                    ));
                }
                store
                    .put_if(&index, Bytes::from_static(b"{2}"), &seen, &json)
                    .await?;
                if !matches!(
                    store
                        .put_if(&index, Bytes::from_static(b"{3}"), &seen, &json)
                        .await,
                    Err(BlobError::Changed(_))
                ) {
                    return Err(BlobError::Config(
                        "an update over a stale copy (If-Match) went through: the store ignores \
                         it, and two servers could undo each other's cloud save indexes"
                            .into(),
                    ));
                }
                Ok("create-only and if-match both enforced".to_string())
            }
            .await,
        );
    }
    if options.quick {
        step("multipart upload", Ok("skipped (--quick)".into()));
    } else {
        step(
            "multipart upload",
            async {
                let bytes = pattern(BIG);
                let file = std::env::temp_dir().join(format!("h2-store-check-{id}"));
                tokio::fs::write(&file, &bytes).await?;
                let put = store.put_file(&big, &file, &Meta::default()).await;
                let _ = std::fs::remove_file(&file);
                put?;
                let got = store
                    .get(&big)
                    .await?
                    .ok_or_else(|| BlobError::Config("the upload is not there".into()))?;
                if sha(&got) != sha(&bytes) {
                    return Err(BlobError::Config(format!(
                        "read back {} bytes that differ from the {BIG} uploaded",
                        got.len()
                    )));
                }
                Ok(format!(
                    "{} MiB uploaded in parts and read back identical",
                    BIG >> 20
                ))
            }
            .await,
        );
    }
    if let Some(url) = &options.public_url {
        let target = format!("{}/{small}", url.trim_end_matches('/'));
        let outcome = async {
            let resp = reqwest::get(&target)
                .await
                .map_err(|e| BlobError::Config(format!("fetching {target}: {e}")))?;
            let status = resp.status();
            if !status.is_success() {
                return Err(BlobError::Config(format!(
                    "GET {target} answered {status}: is public access on for the bucket, and \
                     is --public-url its custom domain or r2.dev address?"
                )));
            }
            let got = resp
                .bytes()
                .await
                .map_err(|e| BlobError::Config(format!("reading {target}: {e}")))?;
            if got != body {
                return Err(BlobError::Config(format!(
                    "{target} served {} bytes that are not the test object",
                    got.len()
                )));
            }
            Ok(format!("{target} serves it without credentials"))
        }
        .await;
        step("public URL", outcome);
    }
    // Always last, and always tried: leaves nothing behind.
    step(
        "delete",
        async {
            for key in [&small, &index, &big] {
                store.delete(key).await?;
            }
            if store.head(&small).await?.is_some() {
                return Err(BlobError::Config(
                    "the object is still there after its delete".into(),
                ));
            }
            let left = store.list(&format!("{base}/")).await?;
            if left.is_empty() {
                Ok("scratch objects removed, prefix empty".to_string())
            } else {
                Err(BlobError::Config(format!(
                    "{} object(s) left under .h2-store-check/{id}/ (delete them by hand)",
                    left.len()
                )))
            }
        }
        .await,
    );
    all_ok
}

/// `store check`'s arguments (after `store check`); progress goes to `out`.
pub async fn run_with(
    args: Vec<String>,
    env: &dyn Fn(&str) -> Option<String>,
    out: &mut dyn FnMut(String),
) -> Result<()> {
    let mut args = crate::standalone::Args(args);
    if args.flag("--help") || args.flag("-h") {
        bail!("{USAGE}");
    }
    let blobs = args.take("--blobs")?.or_else(|| env("H2_LIVE_BLOBS"));
    let public = args.flag("--public");
    let public_url = args.take("--public-url")?;
    let quick = args.flag("--quick");
    if let Some(extra) = args.0.first() {
        bail!("unexpected {extra}\n\n{USAGE}");
    }
    let Some(blobs) = blobs else {
        bail!("--blobs (or $H2_LIVE_BLOBS) names the store to check\n\n{USAGE}");
    };
    let location = Location::parse(&blobs)?;
    let mut secrets = Vec::new();
    if let Location::S3 { .. } = location {
        let missing: Vec<&str> = [
            crate::blobs::ENV_ENDPOINT,
            crate::blobs::ENV_ACCESS_KEY,
            crate::blobs::ENV_SECRET_KEY,
        ]
        .into_iter()
        .filter(|n| S3Config::from_env_one(env, n, public).is_none())
        .collect();
        if !missing.is_empty() {
            bail!(
                "not set: {} (see docs/live/live-hosting.md, \"Check the bucket\")",
                missing.join(", ")
            );
        }
        let c = S3Config::from_env(env, public)?;
        out(format!(
            "store check: {blobs} (endpoint {}, region {})",
            c.endpoint, c.region
        ));
        secrets = vec![c.secret_access_key, c.access_key_id];
    } else {
        out(format!("store check: {blobs}"));
    }
    let store = BlobStore::open(&location, env, public)?;
    let options = Options { public_url, quick };
    let (mut passed, mut failed) = (0usize, 0usize);
    run_steps(&store, &options, &secrets, |name, outcome| match outcome {
        Ok(msg) => {
            passed += 1;
            out(format!("  [ ok ]  {name:<22} {msg}"));
        }
        Err(msg) => {
            failed += 1;
            out(format!("  [FAIL]  {name:<22} {msg}"));
        }
    })
    .await;
    if failed > 0 {
        bail!("store check: {failed} of {} steps failed", passed + failed);
    }
    out(format!(
        "store check: all {passed} steps passed; {blobs} is ready for --blobs"
    ));
    Ok(())
}

/// [`run_with`], printing to stdout.
pub async fn run(args: Vec<String>, env: &dyn Fn(&str) -> Option<String>) -> Result<()> {
    run_with(args, env, &mut |line| println!("{line}")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn steps(store: &BlobStore, options: &Options) -> (bool, Vec<(String, Outcome)>) {
        let mut seen = Vec::new();
        let ok = run_steps(store, options, &[], |n, o| seen.push((n.to_string(), o))).await;
        (ok, seen)
    }

    #[tokio::test]
    async fn a_working_store_passes_every_step_and_is_left_empty() {
        for store in [
            BlobStore::memory(),
            BlobStore::dir(&crate::blobs::tests::temp_dir("check")).unwrap(),
        ] {
            let (ok, seen) = steps(&store, &Options::default()).await;
            assert!(ok, "{seen:#?}");
            let names: Vec<_> = seen.iter().map(|s| s.0.as_str()).collect();
            assert_eq!(
                names,
                [
                    "write",
                    "read",
                    "head and content type",
                    "list",
                    "range read",
                    "conditional write",
                    "multipart upload",
                    "delete"
                ]
            );
            assert!(store.list(".h2-store-check/").await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn quick_skips_the_multipart_upload() {
        let store = BlobStore::memory();
        let options = Options {
            quick: true,
            ..Options::default()
        };
        let (ok, seen) = steps(&store, &options).await;
        assert!(ok);
        assert!(
            seen.iter().any(|(n, o)| n == "multipart upload"
                && o.as_ref().is_ok_and(|m| m.starts_with("skipped")))
        );
    }

    #[test]
    fn errors_are_scrubbed_and_explained() {
        let secrets = vec!["hunter2-secret".to_string(), "AKIAKEY123".to_string()];
        assert_eq!(
            scrub("bad hunter2-secret for AKIAKEY123", &secrets),
            "bad *** for ***"
        );
        let denied = explain(&BlobError::Config("Forbidden: 403 AccessDenied".into()));
        assert!(denied.contains("Object Read & Write"), "{denied}");
        let missing = explain(&BlobError::Config("NoSuchBucket".into()));
        assert!(missing.contains("bucket name"), "{missing}");
    }

    #[tokio::test]
    async fn arguments_are_checked_before_anything_is_tried() {
        let none = |_: &str| None;
        let mut sink = |_: String| {};
        let err = run_with(vec![], &none, &mut sink).await.unwrap_err();
        assert!(err.to_string().contains("--blobs"), "{err}");
        let err = run_with(vec!["--blobs".into(), "s3://b".into()], &none, &mut sink)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("H2_LIVE_S3_ENDPOINT")
                && err.contains("H2_LIVE_S3_ACCESS_KEY_ID")
                && err.contains("H2_LIVE_S3_SECRET_ACCESS_KEY"),
            "{err}"
        );
        assert!(
            run_with(vec!["--nope".into()], &none, &mut sink)
                .await
                .is_err()
        );
    }
}
