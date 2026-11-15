//! Checking events read back from a server, as a reader would
//! (docs/live-security.md §2): the stored JWS opens to exactly the event's
//! fields, verifies under the author's key, and extends its stream's hash
//! chain. Key lookup is the caller's, so this stays pure.

use std::collections::HashMap;

use crate::WireEvent;
use crate::sign::{PublicKey, SignedEvent, chain_hash};

/// What is wrong with one event.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    #[error("the JWS does not open: {0}")]
    Malformed(String),
    #[error("the event's {0} differs from its signed body")]
    Mismatch(&'static str),
    #[error("no key {kid} for {author} at this position")]
    UnknownKey { author: String, kid: String },
    #[error("the signature does not verify")]
    BadSignature,
    #[error("hash is {got}, the chain wants {want}")]
    BrokenChain { got: String, want: String },
    #[error("version {got} follows version {prev} of the stream")]
    VersionGap { got: u64, prev: u64 },
}

/// Checks the JWS against the event's own fields and, given its key, the
/// signature. `key` is `None` when the author's key at this position is
/// unknown.
pub fn check_signature(e: &WireEvent, key: Option<&PublicKey>) -> Result<(), Problem> {
    let body = SignedEvent::body_at(&e.jws, e.jws_index)
        .map_err(|err| Problem::Malformed(err.to_string()))?;
    for (field, same) in [
        ("stream", body.stream == e.stream),
        ("type", body.kind == e.kind),
        ("data", body.data == e.data),
        ("author", body.author == e.author),
        ("kid", body.kid == e.kid),
        ("at", body.at == e.at),
    ] {
        if !same {
            return Err(Problem::Mismatch(field));
        }
    }
    let key = key.ok_or_else(|| Problem::UnknownKey {
        author: e.author.clone(),
        kid: e.kid.clone(),
    })?;
    if key.kid() != e.kid {
        return Err(Problem::UnknownKey {
            author: e.author.clone(),
            kid: e.kid.clone(),
        });
    }
    let jws = crate::sign::Jws::parse(&e.jws).map_err(|err| Problem::Malformed(err.to_string()))?;
    jws.verify(key).map_err(|_| Problem::BadSignature)
}

/// The hash chains of the streams seen so far, in log order.
#[derive(Debug, Default)]
pub struct Chains {
    last: HashMap<String, (u64, String)>,
}

/// How an event sits on its chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// Its hash follows from the previous event (or it is the stream's
    /// first, version 0).
    Checked,
    /// The first event seen of a stream that started earlier: taken as
    /// the anchor, since its predecessor was not read.
    Anchored,
}

impl Chains {
    pub fn check(&mut self, e: &WireEvent) -> Result<Link, Problem> {
        let prev = self.last.get(&e.stream);
        let (link, prev_hash) = match prev {
            Some((v, _)) if *v + 1 != e.version => {
                return Err(Problem::VersionGap {
                    got: e.version,
                    prev: *v,
                });
            }
            Some((_, h)) => (Link::Checked, Some(h.as_str())),
            None if e.version == 0 => (Link::Checked, None),
            None => (Link::Anchored, None),
        };
        if link == Link::Checked {
            let want = chain_hash(prev_hash, &e.jws);
            if want != e.hash {
                return Err(Problem::BrokenChain {
                    got: e.hash.clone(),
                    want,
                });
            }
        }
        self.last
            .insert(e.stream.clone(), (e.version, e.hash.clone()));
        Ok(link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::{EventBody, KeyPair, sign_batch};
    use serde_json::json;

    fn wire(
        key: &KeyPair,
        version: u64,
        prev: Option<&str>,
        kind: &str,
        data: serde_json::Value,
    ) -> WireEvent {
        let body = EventBody::new(
            key,
            "acct:0009000000000001@t",
            "s",
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        );
        let jws = body.sign(key);
        WireEvent {
            position: version + 1,
            stream: body.stream,
            version,
            kind: body.kind,
            at: body.at,
            data: body.data,
            author: body.author,
            kid: body.kid,
            hash: chain_hash(prev, &jws),
            jws,
            jws_index: None,
        }
    }

    #[test]
    fn a_good_chain_checks() {
        let key = KeyPair::generate();
        let a = wire(&key, 0, None, "A", json!({"n": 1}));
        let b = wire(&key, 1, Some(&a.hash), "B", json!({"n": 2}));
        let mut chains = Chains::default();
        for e in [&a, &b] {
            check_signature(e, Some(key.public())).unwrap();
            assert_eq!(chains.check(e), Ok(Link::Checked));
        }
    }

    #[test]
    fn tampering_is_found() {
        let key = KeyPair::generate();
        let mut a = wire(&key, 0, None, "A", json!({"n": 1}));
        assert_eq!(
            check_signature(&a, None),
            Err(Problem::UnknownKey {
                author: a.author.clone(),
                kid: a.kid.clone()
            })
        );
        assert!(matches!(
            check_signature(&a, Some(KeyPair::generate().public())),
            Err(Problem::UnknownKey { .. })
        ));
        a.data = json!({"n": 2});
        assert_eq!(
            check_signature(&a, Some(key.public())),
            Err(Problem::Mismatch("data"))
        );

        let a = wire(&key, 0, None, "A", json!({"n": 1}));
        let mut b = wire(&key, 1, None, "B", json!({}));
        let mut chains = Chains::default();
        chains.check(&a).unwrap();
        assert!(matches!(chains.check(&b), Err(Problem::BrokenChain { .. })));
        b.version = 5;
        assert_eq!(
            chains.check(&b),
            Err(Problem::VersionGap { got: 5, prev: 0 })
        );
        // A stream first seen past its first event anchors there.
        let mut fresh = Chains::default();
        assert_eq!(fresh.check(&b), Ok(Link::Anchored));
    }

    #[test]
    fn batch_elements_check_against_their_index() {
        let key = KeyPair::generate();
        let bodies: Vec<EventBody> = (0..2)
            .map(|i| {
                EventBody::new(
                    &key,
                    "acct:0009000000000001@t",
                    "s",
                    "K",
                    json!({"i": i}),
                    time::OffsetDateTime::now_utc(),
                )
            })
            .collect();
        let jws = sign_batch(&key, &bodies);
        let b = &bodies[1];
        let mut e = WireEvent {
            position: 2,
            stream: b.stream.clone(),
            version: 2,
            kind: b.kind.clone(),
            at: b.at.clone(),
            data: b.data.clone(),
            author: b.author.clone(),
            kid: b.kid.clone(),
            jws,
            jws_index: Some(1),
            hash: String::new(),
        };
        check_signature(&e, Some(key.public())).unwrap();
        e.jws_index = Some(0);
        assert_eq!(
            check_signature(&e, Some(key.public())),
            Err(Problem::Mismatch("data"))
        );
    }
}
