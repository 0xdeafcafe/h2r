//! Packages over Live (docs/editor/studio-redesign.md, "Live: sharing,
//! signing and credit"; docs/live/live-api.md, "Packages").
//!
//! A package's files go into the content store by SHA-256
//! (`PUT /v1/content/{sha256}`); its manifest and lock go up as one
//! [`PackageDocument`], signed by the author's own key as a
//! [`PACKAGE_PUBLISHED`] event on [`stream`]. The upload check is
//! [`check`], over what the home's store says ([`Holdings`]): both homes
//! call it, so they refuse the same documents with the same words.
use serde::{Deserialize, Serialize};

pub use content::package::{IdError, Kind, Lock, Manifest, PackageId};

use crate::maps::{Credit, Sha256, Who};
use crate::Xuid;

/// `PackagePublished`: a package's manifest and lock, signed by its
/// author on [`stream`]. Data: [`PackageDocument`].
pub const PACKAGE_PUBLISHED: &str = "PackagePublished";
/// `PackageCredited`: an admin set (or cleared) who made a package, over
/// what its publisher signed (server-signed, on [`stream`]). The manifest,
/// its `PackageId` and the signature are untouched. Data: [`Credited`].
pub const PACKAGE_CREDITED: &str = "PackageCredited";
/// `ModelUploadsSet`: an admin approved an account to publish packages
/// with custom models, or took it back (server-signed, on the account's
/// stream). Data: [`ModelUploadsSet`].
pub const MODEL_UPLOADS_SET: &str = "ModelUploadsSet";

/// Every event type of this module; only the server's own endpoints write
/// them.
pub const KINDS: [&str; 3] = [PACKAGE_PUBLISHED, PACKAGE_CREDITED, MODEL_UPLOADS_SET];

/// The prefix of every package's stream. Not `package-`: that is a map
/// package's (`maps::package_stream`).
pub const STREAM_PREFIX: &str = "pkg-";

/// The stream a package's events are on: `pkg-<PackageId>`.
pub fn stream(id: &PackageId) -> String {
    format!("{STREAM_PREFIX}{id}")
}

/// The item kinds that carry a custom model, which need the uploader's
/// `model_uploads` flag (docs/game/forge-features.md, "Custom models on
/// Live").
pub const MODEL_KINDS: [Kind; 2] = [Kind::Model, Kind::Animation];

/// Item kinds only first-party packages use: Live refuses them.
pub const FIRST_PARTY_KINDS: [Kind; 1] = [Kind::Kit];

/// The most files one package's lock names.
pub const MAX_FILES: usize = 4096;

/// The document a package's author signs: the manifest, its lock, and
/// optionally who made it when the publisher did not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDocument {
    pub manifest: Manifest,
    pub lock: Lock,
    /// The publisher's credit. Over the manifest's `[credit]`, under an
    /// admin's ([`Credited`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<Credit>,
}

impl PackageDocument {
    pub fn id(&self) -> Result<PackageId, IdError> {
        PackageId::of(&self.manifest, &self.lock)
    }

    /// The manifest's `[credit]` as Live's [`Credit`]; None when empty.
    pub fn manifest_credit(&self) -> Option<Credit> {
        let c = &self.manifest.credit;
        let c = Credit {
            authors: c.authors.clone(),
            release: c.release.clone(),
            released: c.released.clone(),
            source_url: c.source_url.clone(),
            notes: c.notes.clone(),
        }
        .tidy();
        (!c.is_empty()).then_some(c)
    }

    /// The publisher's credit: the signed `credit`, else the manifest's.
    pub fn credit(&self) -> Option<Credit> {
        self.credit
            .clone()
            .map(Credit::tidy)
            .filter(|c| !c.is_empty())
            .or_else(|| self.manifest_credit())
    }

    /// Whether an item carries a custom model ([`MODEL_KINDS`]).
    pub fn has_model(&self) -> bool {
        self.manifest
            .items
            .iter()
            .any(|i| MODEL_KINDS.contains(&i.kind))
    }
}

/// What a home's store says about a file and an account, for [`check`].
pub trait Holdings {
    /// The hash is on the operator's retail list.
    fn is_retail(&self, h: &Sha256) -> bool;
    /// What the store has under the hash.
    fn file(&self, h: &Sha256) -> FileState;
    /// The account's `model_uploads` flag, as the log says now.
    fn model_uploads(&self, xuid: Xuid) -> bool;
}

/// A file in the content store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    Stored,
    Missing,
    /// An admin hid, deleted or blocked it.
    TakenDown,
}

/// Why a package document is refused. Each names what is wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("the manifest: {0}")]
    Manifest(String),
    #[error("the lock: {0}")]
    Lock(String),
    #[error("the credit: {0}")]
    Credit(String),
    #[error("the stream is {got}, the PackageId says {want}")]
    Stream { got: String, want: String },
    #[error("item {index} is a `{kind}` item, which only first-party packages use")]
    FirstParty { index: usize, kind: String },
    #[error(
        "model_uploads_not_approved: item {index} is a `{kind}` item, and this account is not approved for custom models"
    )]
    ModelUploads { index: usize, kind: String },
    #[error("{file} ({sha256}) is a retail file: it is never shared")]
    Retail { file: String, sha256: Sha256 },
    #[error("an admin took down {file} ({sha256})")]
    TakenDown { file: String, sha256: Sha256 },
    #[error("upload these files first: {}", .0.join(", "))]
    Missing(Vec<String>),
}

impl Refusal {
    /// The HTTP status class: 400, 403, 409 or 410.
    pub fn code(&self) -> crate::ErrorCode {
        use crate::ErrorCode::*;
        match self {
            Self::Manifest(_) | Self::Lock(_) | Self::Credit(_) | Self::Stream { .. } => Invalid,
            Self::FirstParty { .. } | Self::ModelUploads { .. } | Self::Retail { .. } => Forbidden,
            Self::TakenDown { .. } => Gone,
            Self::Missing(_) => Conflict,
        }
    }
}

fn kind_word(k: Kind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The upload check, the same in both homes: the manifest validates; the
/// lock names exactly the manifest's files; the document's id is the
/// stream's; no first-party item; a model item only from an approved
/// account; every locked hash off the retail list, not taken down and
/// stored. The signature is checked before this, by the caller.
pub fn check(
    doc: &PackageDocument,
    stream_name: &str,
    uploader: Xuid,
    store: &dyn Holdings,
) -> Result<PackageId, Refusal> {
    let m = &doc.manifest;
    m.validate().map_err(|e| Refusal::Manifest(e.to_string()))?;
    if doc.lock.format != content::package::PACKAGE_FORMAT {
        return Err(Refusal::Lock(format!(
            "format {} is not supported (this build reads {})",
            doc.lock.format,
            content::package::PACKAGE_FORMAT
        )));
    }
    if doc.lock.files.len() > MAX_FILES {
        return Err(Refusal::Lock(format!("at most {MAX_FILES} files")));
    }
    if let Some(f) = m.files().find(|f| !doc.lock.files.contains_key(*f)) {
        return Err(Refusal::Lock(format!("no hash for {f:?}")));
    }
    if let Some(f) = doc
        .lock
        .files
        .keys()
        .find(|f| !m.files().any(|n| n == f.as_str()))
    {
        return Err(Refusal::Lock(format!(
            "{f:?} is not a file the manifest names"
        )));
    }
    let id = doc.id().map_err(|e| Refusal::Lock(e.to_string()))?;
    if stream_name != stream(&id) {
        return Err(Refusal::Stream {
            got: stream_name.to_owned(),
            want: stream(&id),
        });
    }
    if let Some(c) = &doc.credit {
        c.validate().map_err(Refusal::Credit)?;
    }
    if let Some(c) = doc.manifest_credit() {
        c.validate().map_err(Refusal::Credit)?;
    }
    for (index, item) in m.items.iter().enumerate() {
        if FIRST_PARTY_KINDS.contains(&item.kind) {
            return Err(Refusal::FirstParty {
                index,
                kind: kind_word(item.kind),
            });
        }
    }
    if let Some((index, item)) = m
        .items
        .iter()
        .enumerate()
        .find(|(_, i)| MODEL_KINDS.contains(&i.kind))
        && !store.model_uploads(uploader)
    {
        return Err(Refusal::ModelUploads {
            index,
            kind: kind_word(item.kind),
        });
    }
    let mut missing = Vec::new();
    for (file, h) in &doc.lock.files {
        if store.is_retail(h) {
            return Err(Refusal::Retail {
                file: file.clone(),
                sha256: *h,
            });
        }
        match store.file(h) {
            FileState::Stored => {}
            FileState::TakenDown => {
                return Err(Refusal::TakenDown {
                    file: file.clone(),
                    sha256: *h,
                });
            }
            FileState::Missing => missing.push(format!("{file} ({h})")),
        }
    }
    if !missing.is_empty() {
        return Err(Refusal::Missing(missing));
    }
    Ok(id)
}

/// `PackageCredited` data. `credit` None: the admin's credit is cleared
/// and the publisher's shows again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credited {
    pub id: PackageId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<Credit>,
    /// The admin's account (None: the operator's key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Xuid>,
}

/// The longest note an admin keeps with a `model_uploads` change.
pub const MAX_NOTE: usize = 500;

/// `ModelUploadsSet` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUploadsSet {
    pub xuid: Xuid,
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Xuid>,
}

/// A package as listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSummary {
    pub id: PackageId,
    /// The manifest's `[package] id`.
    pub slug: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The account that published it ("uploaded by"), never the author.
    pub publisher: Who,
    /// Who made it: an admin's credit, else the publisher's signed
    /// credit, else the manifest's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<Credit>,
    /// The item kinds, each once, in manifest order.
    pub kinds: Vec<String>,
    pub items: usize,
    /// Any item changes what the simulation reads.
    pub sim: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// The files' bytes.
    pub size: u64,
    pub published_at: String,
    pub position: u64,
}

/// `GET /v1/packages/{id}`: the summary, the signed document as published
/// (the compact JWS, which a client verifies against the publisher's key
/// on install), and the manifest and lock it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageAnswer {
    pub package: PackageSummary,
    pub manifest: Manifest,
    pub lock: Lock,
    /// The publisher's `PackagePublished` event.
    pub jws: String,
    /// The publisher's account id (the event's `author`), whose key
    /// signed `jws`.
    pub author: String,
    pub position: u64,
}

/// `GET /v1/packages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageList {
    pub items: Vec<PackageSummary>,
    pub position: u64,
}

/// Why a client refuses a package it fetched (`package.install`, PK14).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("the signature: {0}")]
    Signature(String),
    #[error("the signed event is a {0}, not a {PACKAGE_PUBLISHED}")]
    Kind(String),
    #[error("the signer {signer} is not the answer's author {author}")]
    Author { signer: String, author: String },
    #[error("the manifest or lock differs from what was signed")]
    Altered,
    #[error("the signed document: {0}")]
    Document(String),
    #[error("the PackageId is {got}, the answer says {want}")]
    Id { got: PackageId, want: PackageId },
    #[error("{file}: the bytes hash to {got}, the lock says {want}")]
    File {
        file: String,
        got: Sha256,
        want: Sha256,
    },
    #[error("{0}: the lock names no such file")]
    Unlocked(String),
}

/// A client's check of a fetched package, before it writes anything: the
/// `jws` verifies under the publisher's key, is a [`PACKAGE_PUBLISHED`]
/// by the answer's author, holds exactly the answer's manifest and lock,
/// and gives the answer's `PackageId`. The signed document is returned.
pub fn verify_answer(
    answer: &PackageAnswer,
    key: &crate::sign::PublicKey,
) -> Result<PackageDocument, InstallError> {
    let event = crate::sign::SignedEvent::open(&answer.jws)
        .map_err(|e| InstallError::Signature(e.to_string()))?;
    event
        .verify(key)
        .map_err(|e| InstallError::Signature(e.to_string()))?;
    let b = &event.body;
    if b.kind != PACKAGE_PUBLISHED {
        return Err(InstallError::Kind(b.kind.clone()));
    }
    if b.author != answer.author {
        return Err(InstallError::Author {
            signer: b.author.clone(),
            author: answer.author.clone(),
        });
    }
    let doc: PackageDocument = serde_json::from_value(b.data.clone())
        .map_err(|e| InstallError::Document(e.to_string()))?;
    if doc.manifest != answer.manifest || doc.lock != answer.lock {
        return Err(InstallError::Altered);
    }
    let got = doc.id().map_err(|e| InstallError::Document(e.to_string()))?;
    if got != answer.package.id || b.stream != stream(&got) {
        return Err(InstallError::Id {
            got,
            want: answer.package.id,
        });
    }
    Ok(doc)
}

/// A downloaded file against the lock: its SHA-256 is the locked one.
pub fn verify_file(lock: &Lock, file: &str, bytes: &[u8]) -> Result<(), InstallError> {
    let want = *lock
        .files
        .get(file)
        .ok_or_else(|| InstallError::Unlocked(file.to_owned()))?;
    let got = Sha256::of(bytes);
    if got != want {
        return Err(InstallError::File {
            file: file.to_owned(),
            got,
            want,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashSet};

    struct Fake {
        retail: HashSet<Sha256>,
        stored: HashSet<Sha256>,
        down: HashSet<Sha256>,
        approved: bool,
    }

    impl Holdings for Fake {
        fn is_retail(&self, h: &Sha256) -> bool {
            self.retail.contains(h)
        }
        fn file(&self, h: &Sha256) -> FileState {
            if self.down.contains(h) {
                FileState::TakenDown
            } else if self.stored.contains(h) {
                FileState::Stored
            } else {
                FileState::Missing
            }
        }
        fn model_uploads(&self, _: Xuid) -> bool {
            self.approved
        }
    }

    fn doc(kind: &str) -> PackageDocument {
        let manifest = Manifest::from_toml(&format!(
            "[package]\nformat = 1\nid = \"mine\"\nname = \"Mine\"\n\n\
             [credit]\nauthors = [\"Manifest Author\"]\n\n\
             [[item]]\nkind = \"{kind}\"\nsource = \"a.yaml\"\n"
        ))
        .unwrap();
        let lock = Lock::new(BTreeMap::from([("a.yaml".to_string(), Sha256([7; 32]))]));
        PackageDocument {
            manifest,
            lock,
            credit: None,
        }
    }

    fn all_stored() -> Fake {
        Fake {
            retail: HashSet::new(),
            stored: HashSet::from([Sha256([7; 32])]),
            down: HashSet::new(),
            approved: false,
        }
    }

    fn run(d: &PackageDocument, f: &Fake) -> Result<PackageId, Refusal> {
        check(d, &stream(&d.id().unwrap()), Xuid(1), f)
    }

    #[test]
    fn a_tag_package_with_its_files_stored_passes() {
        let d = doc("tag");
        assert_eq!(run(&d, &all_stored()), Ok(d.id().unwrap()));
    }

    #[test]
    fn each_refusal_is_named() {
        let d = doc("tag");
        let mut f = all_stored();
        f.stored.clear();
        assert!(matches!(run(&d, &f), Err(Refusal::Missing(m)) if m[0].starts_with("a.yaml")));

        let mut f = all_stored();
        f.retail.insert(Sha256([7; 32]));
        assert!(matches!(run(&d, &f), Err(Refusal::Retail { file, .. }) if file == "a.yaml"));

        let mut f = all_stored();
        f.down.insert(Sha256([7; 32]));
        assert!(matches!(run(&d, &f), Err(Refusal::TakenDown { .. })));

        assert!(matches!(
            check(&d, "pkg-00", Xuid(1), &all_stored()),
            Err(Refusal::Stream { .. })
        ));
    }

    #[test]
    fn a_lock_must_name_exactly_the_manifests_files() {
        let mut d = doc("tag");
        d.lock.files.insert("extra.bin".into(), Sha256([8; 32]));
        let s = stream(&doc("tag").id().unwrap());
        assert!(matches!(
            check(&d, &s, Xuid(1), &all_stored()),
            Err(Refusal::Lock(e)) if e.contains("extra.bin")
        ));
        let mut d = doc("tag");
        d.lock.files.clear();
        assert!(matches!(
            check(&d, &s, Xuid(1), &all_stored()),
            Err(Refusal::Lock(e)) if e.contains("a.yaml")
        ));
    }

    #[test]
    fn a_model_item_needs_the_flag() {
        for kind in ["model", "animation"] {
            let d = doc(kind);
            let r = run(&d, &all_stored());
            assert!(
                matches!(&r, Err(Refusal::ModelUploads { kind: k, .. }) if k == kind),
                "{r:?}"
            );
            assert_eq!(r.unwrap_err().code(), crate::ErrorCode::Forbidden);
            let mut f = all_stored();
            f.approved = true;
            assert!(run(&d, &f).is_ok());
        }
    }

    #[test]
    fn a_kit_item_is_first_party_only() {
        let manifest = Manifest::from_toml(
            "[package]\nformat = 1\nid = \"mine\"\nname = \"Mine\"\n\n[[item]]\nkind = \"kit\"\n",
        )
        .unwrap();
        let d = PackageDocument {
            manifest,
            lock: Lock::new(BTreeMap::new()),
            credit: None,
        };
        assert!(matches!(run(&d, &all_stored()), Err(Refusal::FirstParty { .. })));
    }

    fn answer_for(d: &PackageDocument, key: &crate::sign::KeyPair, author: &str) -> PackageAnswer {
        let id = d.id().unwrap();
        let jws = crate::sign::EventBody::at_unix(
            key,
            author,
            &stream(&id),
            PACKAGE_PUBLISHED,
            serde_json::to_value(d).unwrap(),
            1_700_000_000,
        )
        .sign(key);
        PackageAnswer {
            package: PackageSummary {
                id,
                slug: d.manifest.package.id.clone(),
                name: d.manifest.package.name.clone(),
                description: None,
                publisher: Who {
                    xuid: Xuid(1),
                    gamertag: "Maker".into(),
                },
                credit: d.credit(),
                kinds: vec!["tag".into()],
                items: 1,
                sim: true,
                requires: Vec::new(),
                features: Vec::new(),
                size: 0,
                published_at: String::new(),
                position: 1,
            },
            manifest: d.manifest.clone(),
            lock: d.lock.clone(),
            jws,
            author: author.into(),
            position: 1,
        }
    }

    #[test]
    fn an_install_verifies_the_signature_the_document_and_each_file() {
        let key = crate::sign::KeyPair::generate();
        let d = doc("tag");
        let a = answer_for(&d, &key, "acct-1");
        assert_eq!(verify_answer(&a, key.public()), Ok(d.clone()));

        let stranger = crate::sign::KeyPair::generate();
        assert!(matches!(
            verify_answer(&a, stranger.public()),
            Err(InstallError::Signature(_))
        ));
        let mut altered = a.clone();
        altered.lock.files.insert("a.yaml".into(), Sha256([9; 32]));
        assert_eq!(verify_answer(&altered, key.public()), Err(InstallError::Altered));
        let mut wrong = a.clone();
        wrong.package.id = PackageId([0; 32]);
        assert!(matches!(verify_answer(&wrong, key.public()), Err(InstallError::Id { .. })));
        let mut other = a.clone();
        other.author = "acct-2".into();
        assert!(matches!(verify_answer(&other, key.public()), Err(InstallError::Author { .. })));

        let lock = Lock::new(BTreeMap::from([("a.yaml".to_string(), Sha256::of(b"abc"))]));
        assert_eq!(verify_file(&lock, "a.yaml", b"abc"), Ok(()));
        assert!(matches!(verify_file(&lock, "a.yaml", b"abd"), Err(InstallError::File { .. })));
        assert!(matches!(verify_file(&lock, "b.yaml", b"abc"), Err(InstallError::Unlocked(_))));
    }

    #[test]
    fn the_signed_credit_outranks_the_manifests() {
        let mut d = doc("tag");
        assert_eq!(d.credit().unwrap().authors, ["Manifest Author"]);
        d.credit = Some(Credit::authors(["Publisher Says"]));
        assert_eq!(d.credit().unwrap().authors, ["Publisher Says"]);
        // Credit is display data: the id does not move.
        assert_eq!(d.id(), doc("tag").id());
    }
}
