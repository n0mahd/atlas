//! A bounded model check of how atlasd accepts updates.
//!
//! A compromised hub may serve any release it likes, in any order, as often as it likes.
//! This explores every state atlasd can reach when the hub picks from a small universe of
//! releases (numbers 1–3, two contents, three owner-key lists, signed by either of the
//! owner's keys or by an attacker's), using the real [`release::check`] and the same state
//! update `pass::run` makes when a release applies or fails to apply. Every reachable
//! transition must keep the invariants below. Small scope, but exhaustive: any sequence of
//! up to the depth bound is covered, not just the ones someone thought to write.
//!
//! Invariants:
//! 1. A release is accepted only if a key trusted at that moment signed it, or it is the
//!    very release already applied (same number, same bytes).
//! 2. The applied number never goes down.
//! 3. One number never stands for two different manifests.
//! 4. Trusted keys change only by applying a release signed by a key trusted before it.
//! 5. The attacker's key never becomes trusted.

use std::collections::{BTreeSet, HashSet, VecDeque};

use atlas_client::config::State;
use atlas_client::release;
use atlas_core::release::{FORMAT, Manifest, OwnerKey, Signature, SignedRelease};
use atlas_verify::encode;
use p256::ecdsa::SigningKey;
use sha2::{Digest as _, Sha256};

const HUB: &str = "https://hub.example";
const RP: &str = "hub.example";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Who {
    K1,
    K2,
    Attacker,
}

fn signing(who: Who) -> SigningKey {
    let seed = match who {
        Who::K1 => 1u8,
        Who::K2 => 2,
        Who::Attacker => 99,
    };
    SigningKey::from_slice(&Sha256::digest([b'm', seed])).unwrap()
}

fn cred(who: Who) -> Vec<u8> {
    vec![who as u8 + 1; 16]
}

fn owner_key(who: Who) -> OwnerKey {
    OwnerKey {
        credential_id: encode(&cred(who)),
        alg: "es256".into(),
        public_key: encode(signing(who).verifying_key().to_sec1_point(false).as_bytes()),
        name: format!("{who:?}"),
    }
}

fn who_is(k: &OwnerKey) -> Who {
    [Who::K1, Who::K2, Who::Attacker]
        .into_iter()
        .find(|w| owner_key(*w).credential_id == k.credential_id)
        .unwrap()
}

/// A WebAuthn assertion over `challenge`, as a browser and authenticator make it.
fn sign(who: Who, challenge: &[u8]) -> Signature {
    use p256::ecdsa::signature::Signer as _;
    let mut auth = Sha256::digest(RP.as_bytes()).to_vec();
    auth.push(0x05);
    auth.extend_from_slice(&1u32.to_be_bytes());
    let client = format!(
        r#"{{"type":"webauthn.get","challenge":"{}","origin":"{HUB}"}}"#,
        encode(challenge)
    );
    let mut message = auth.clone();
    message.extend_from_slice(&Sha256::digest(client.as_bytes()));
    let sig: p256::ecdsa::Signature = signing(who).sign(&message);
    Signature {
        credential_id: encode(&cred(who)),
        authenticator_data: encode(&auth),
        client_data_json: encode(client.as_bytes()),
        signature: encode(sig.to_der().as_bytes()),
    }
}

/// One release the hub could serve.
struct Offer {
    number: u64,
    signer: Who,
    keys: Vec<Who>,
    digest: String,
    signed: SignedRelease,
}

fn universe() -> Vec<Offer> {
    let key_lists = [vec![Who::K1], vec![Who::K2], vec![Who::K1, Who::K2]];
    let mut out = Vec::new();
    for number in 1..=3u64 {
        for created in [0u64, 1] {
            for keys in &key_lists {
                let manifest = Manifest {
                    format: FORMAT,
                    number,
                    created,
                    hub: HUB.into(),
                    sources: Vec::new(),
                    owner_keys: keys.iter().map(|w| owner_key(*w)).collect(),
                    projects: Vec::new(),
                };
                let bytes = serde_json::to_vec(&manifest).unwrap();
                let digest = Sha256::digest(&bytes)
                    .iter()
                    .fold(String::new(), |mut s, b| {
                        use std::fmt::Write as _;
                        let _ = write!(s, "{b:02x}");
                        s
                    });
                for signer in [Who::K1, Who::K2, Who::Attacker] {
                    out.push(Offer {
                        number,
                        signer,
                        keys: keys.clone(),
                        digest: digest.clone(),
                        signed: SignedRelease {
                            number,
                            manifest: encode(&bytes),
                            signature: sign(signer, &Sha256::digest(&bytes)),
                        },
                    });
                }
            }
        }
    }
    out
}

/// What atlasd keeps, plus the history the invariants need.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Node {
    trusted: BTreeSet<Who>,
    last: Option<u64>,
    digest: Option<String>,
    /// Every (number, digest) ever applied on this path.
    applied: BTreeSet<(u64, String)>,
}

impl Node {
    fn state(&self) -> State {
        State {
            owner_keys: self.trusted.iter().map(|w| owner_key(*w)).collect(),
            last_applied: self.last,
            last_digest: self.digest.clone(),
        }
    }
}

/// How many hub answers in a row the model explores.
const DEPTH: usize = 4;

#[test]
fn no_sequence_of_hub_answers_breaks_the_update_rules() {
    let offers = universe();
    let start = Node {
        trusted: BTreeSet::from([Who::K1]),
        last: None,
        digest: None,
        applied: BTreeSet::new(),
    };
    let mut seen = HashSet::from([start.clone()]);
    let mut queue = VecDeque::from([(start, 0usize)]);
    let (mut transitions, mut accepted) = (0usize, 0usize);

    while let Some((node, depth)) = queue.pop_front() {
        if depth == DEPTH {
            continue;
        }
        let state = node.state();
        for offer in &offers {
            transitions += 1;
            let Ok(checked) = release::check(&offer.signed, &state, HUB) else {
                continue;
            };
            accepted += 1;
            let replay = node.last == Some(offer.number)
                && node.digest.as_deref() == Some(offer.digest.as_str());

            // 1. Signed by a key trusted now, or the very release already applied.
            assert!(
                node.trusted.contains(&offer.signer) || replay,
                "accepted release {} signed by untrusted {:?} in {node:?}",
                offer.number,
                offer.signer
            );
            // 2. Never goes back.
            if let Some(last) = node.last {
                assert!(
                    offer.number >= last,
                    "went back from {last} to {}",
                    offer.number
                );
            }
            // 3. One number, one manifest.
            assert!(
                node.applied
                    .iter()
                    .all(|(n, d)| *n != offer.number || *d == offer.digest),
                "number {} reused with different contents in {node:?}",
                offer.number
            );
            assert_eq!(checked.newer, !replay);

            // Applying can still fail (a write fails, a source is missing): state stays.
            // When it succeeds, pass::run keeps the number and digest, and the keys only
            // for a newer release.
            let mut next = node.clone();
            next.last = Some(checked.manifest.number);
            next.digest = Some(checked.digest.clone());
            next.applied.insert((offer.number, offer.digest.clone()));
            if checked.newer {
                next.trusted = checked.manifest.owner_keys.iter().map(who_is).collect();
                // 4. Keys changed only through a release a trusted key signed.
                assert!(node.trusted.contains(&offer.signer));
                assert_eq!(next.trusted, offer.keys.iter().copied().collect());
            }
            // 5. The attacker never gets in.
            assert!(!next.trusted.contains(&Who::Attacker));

            if seen.insert(next.clone()) {
                queue.push_back((next, depth + 1));
            }
        }
    }

    // The model must actually reach interesting states, or it proves nothing.
    assert!(
        seen.iter().any(|n| n.trusted == BTreeSet::from([Who::K2])),
        "rotation unreachable"
    );
    assert!(
        seen.iter().any(|n| n.last == Some(3)),
        "number 3 unreachable"
    );
    assert!(accepted > 0 && transitions > accepted);
    eprintln!(
        "model: {} states, {transitions} transitions, {accepted} accepted",
        seen.len()
    );
}
