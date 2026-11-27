//! Real cryptography end to end: keys are generated, assertions built and
//! signed here exactly as an authenticator and browser would.

use atlas_verify::{
    Assertion, Expected, KeyAlg, OwnerKey, VerifyError, encode, owner_key_from_cose, verify,
};
use ciborium::Value;
use sha2::{Digest, Sha256};

const RP: &str = "atlas.example.com";
const ORIGIN: &str = "https://atlas.example.com";
const CHALLENGE: &[u8] = b"release 42 digest: 0123456789abcdef0123456789abcdef";
const EXPECTED: Expected<'static> = Expected {
    rp_id: RP,
    origin: ORIGIN,
    challenge: CHALLENGE,
};
const UP_UV: u8 = 0x05;

enum Signer {
    Es256(p256::ecdsa::SigningKey),
    EdDsa(ed25519_dalek::SigningKey),
}

struct Owner {
    id: Vec<u8>,
    signer: Signer,
}

impl Owner {
    fn es256(seed: u8) -> Self {
        let secret = Sha256::digest([b'p', seed]);
        Owner {
            id: vec![seed; 16],
            signer: Signer::Es256(p256::ecdsa::SigningKey::from_slice(&secret).unwrap()),
        }
    }

    fn eddsa(seed: u8) -> Self {
        let secret: [u8; 32] = Sha256::digest([b'e', seed]).into();
        Owner {
            id: vec![seed; 32],
            signer: Signer::EdDsa(ed25519_dalek::SigningKey::from_bytes(&secret)),
        }
    }

    fn key(&self) -> OwnerKey {
        let (alg, raw) = match &self.signer {
            Signer::Es256(k) => (
                KeyAlg::Es256,
                k.verifying_key().to_sec1_point(false).as_bytes().to_vec(),
            ),
            Signer::EdDsa(k) => (KeyAlg::EdDsa, k.verifying_key().to_bytes().to_vec()),
        };
        OwnerKey::from_raw(&self.id, alg, &raw).unwrap()
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        match &self.signer {
            Signer::Es256(k) => {
                use p256::ecdsa::signature::Signer as _;
                let sig: p256::ecdsa::Signature = k.sign(message);
                sig.to_der().as_bytes().to_vec()
            }
            Signer::EdDsa(k) => {
                use ed25519_dalek::Signer as _;
                k.sign(message).to_bytes().to_vec()
            }
        }
    }

    /// Signs `auth_data || SHA-256(client_data)` and packages the assertion.
    fn assert_raw(&self, auth_data: &[u8], client_data: &[u8]) -> Assertion {
        let mut message = auth_data.to_vec();
        message.extend_from_slice(&Sha256::digest(client_data));
        Assertion {
            credential_id: encode(&self.id),
            authenticator_data: encode(auth_data),
            client_data_json: encode(client_data),
            signature: encode(&self.sign(&message)),
        }
    }

    fn assert_with(&self, rp: &str, flags: u8, client_data: &str) -> Assertion {
        self.assert_raw(&auth_data(rp, flags), client_data.as_bytes())
    }

    fn assertion(&self) -> Assertion {
        self.assert_with(
            RP,
            UP_UV,
            &client_data("webauthn.get", CHALLENGE, ORIGIN, ""),
        )
    }
}

fn auth_data(rp: &str, flags: u8) -> Vec<u8> {
    let mut data = Sha256::digest(rp.as_bytes()).to_vec();
    data.push(flags);
    data.extend_from_slice(&7u32.to_be_bytes());
    data
}

fn client_data(type_: &str, challenge: &[u8], origin: &str, extra: &str) -> String {
    format!(
        r#"{{"type":"{type_}","challenge":"{}","origin":"{origin}"{extra}}}"#,
        encode(challenge)
    )
}

fn owners() -> Vec<Owner> {
    vec![Owner::es256(1), Owner::eddsa(2)]
}

fn check(owner: &Owner, assertion: &Assertion) -> Result<(), VerifyError> {
    verify(assertion, &[owner.key()], &EXPECTED)
}

#[test]
fn valid_assertions_pass() {
    let all = owners();
    let keys: Vec<OwnerKey> = all.iter().map(Owner::key).collect();
    for owner in &all {
        assert_eq!(verify(&owner.assertion(), &keys, &EXPECTED), Ok(()));
        let extra = r#","crossOrigin":false,"tokenBinding":{"status":"supported"}"#;
        let a = owner.assert_with(
            RP,
            UP_UV,
            &client_data("webauthn.get", CHALLENGE, ORIGIN, extra),
        );
        assert_eq!(verify(&a, &keys, &EXPECTED), Ok(()));
        // Extra flags (backup eligible/state, extensions) and trailing extension bytes are fine.
        let mut data = auth_data(RP, UP_UV | 0x18 | 0x80);
        data.extend_from_slice(&[0xa0]);
        let cd = client_data("webauthn.get", CHALLENGE, ORIGIN, "");
        assert_eq!(
            check(owner, &owner.assert_raw(&data, cd.as_bytes())),
            Ok(())
        );
    }
}

#[test]
fn padded_base64_is_tolerated() {
    for owner in &owners() {
        let mut a = owner.assertion();
        for field in [
            &mut a.credential_id,
            &mut a.authenticator_data,
            &mut a.client_data_json,
            &mut a.signature,
        ] {
            while field.len() % 4 != 0 {
                field.push('=');
            }
        }
        assert_eq!(check(owner, &a), Ok(()));
    }
}

#[test]
fn high_s_es256_signature_passes() {
    // Authenticators don't normalise s; WebAuthn accepts both forms.
    let owner = Owner::es256(3);
    let mut a = owner.assertion();
    let der = b64(&a.signature);
    let sig = p256::ecdsa::Signature::from_der(&der).unwrap();
    let (r, s) = sig.split_scalars();
    let flipped = p256::ecdsa::Signature::from_scalars(r.to_bytes(), (-*s).to_bytes()).unwrap();
    a.signature = encode(flipped.to_der().as_bytes());
    assert_ne!(a.signature, encode(&der));
    assert_eq!(check(&owner, &a), Ok(()));
}

fn b64(s: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .unwrap()
}

#[test]
fn wrong_type_is_rejected() {
    for owner in &owners() {
        let a = owner.assert_with(
            RP,
            UP_UV,
            &client_data("webauthn.create", CHALLENGE, ORIGIN, ""),
        );
        assert_eq!(check(owner, &a), Err(VerifyError::WrongType));
    }
}

#[test]
fn wrong_challenge_is_rejected() {
    for owner in &owners() {
        let a = owner.assert_with(
            RP,
            UP_UV,
            &client_data("webauthn.get", b"release 41", ORIGIN, ""),
        );
        assert_eq!(check(owner, &a), Err(VerifyError::WrongChallenge));
        // A padded challenge string is not the exact encoding.
        let cd = format!(
            r#"{{"type":"webauthn.get","challenge":"{}=","origin":"{ORIGIN}"}}"#,
            encode(CHALLENGE)
        );
        assert_eq!(
            check(owner, &owner.assert_with(RP, UP_UV, &cd)),
            Err(VerifyError::WrongChallenge)
        );
    }
}

#[test]
fn wrong_origin_is_rejected() {
    for owner in &owners() {
        for origin in [
            "https://evil.example.com",
            "https://atlas.example.com:8443",
            "http://atlas.example.com",
            "https://atlas.example.com/",
        ] {
            let a = owner.assert_with(
                RP,
                UP_UV,
                &client_data("webauthn.get", CHALLENGE, origin, ""),
            );
            assert_eq!(check(owner, &a), Err(VerifyError::WrongOrigin), "{origin}");
        }
    }
}

#[test]
fn cross_origin_is_rejected() {
    for owner in &owners() {
        let cd = client_data("webauthn.get", CHALLENGE, ORIGIN, r#","crossOrigin":true"#);
        assert_eq!(
            check(owner, &owner.assert_with(RP, UP_UV, &cd)),
            Err(VerifyError::WrongOrigin)
        );
    }
}

#[test]
fn wrong_rp_is_rejected() {
    for owner in &owners() {
        let cd = client_data("webauthn.get", CHALLENGE, ORIGIN, "");
        let a = owner.assert_with("example.com", UP_UV, &cd);
        assert_eq!(check(owner, &a), Err(VerifyError::WrongRp));
    }
}

#[test]
fn missing_user_presence_or_verification_is_rejected() {
    for owner in &owners() {
        let cd = client_data("webauthn.get", CHALLENGE, ORIGIN, "");
        assert_eq!(
            check(owner, &owner.assert_with(RP, 0x04, &cd)),
            Err(VerifyError::NotPresent)
        );
        assert_eq!(
            check(owner, &owner.assert_with(RP, 0x00, &cd)),
            Err(VerifyError::NotPresent)
        );
        assert_eq!(
            check(owner, &owner.assert_with(RP, 0x01, &cd)),
            Err(VerifyError::NotVerified)
        );
    }
}

#[test]
fn tampered_authenticator_data_is_rejected() {
    for owner in &owners() {
        let cd = client_data("webauthn.get", CHALLENGE, ORIGIN, "");
        // Signed with UP only, then the UV flag is switched on.
        let mut a = owner.assert_with(RP, 0x01, &cd);
        let mut data = auth_data(RP, 0x01);
        data[32] |= 0x04;
        a.authenticator_data = encode(&data);
        assert_eq!(check(owner, &a), Err(VerifyError::BadSignature));
        // Counter changed.
        let mut a = owner.assertion();
        let mut data = auth_data(RP, UP_UV);
        data[36] ^= 1;
        a.authenticator_data = encode(&data);
        assert_eq!(check(owner, &a), Err(VerifyError::BadSignature));
    }
}

#[test]
fn tampered_client_data_is_rejected() {
    for owner in &owners() {
        let mut a = owner.assertion();
        a.client_data_json =
            encode(client_data("webauthn.get", b"release 43", ORIGIN, "").as_bytes());
        assert_eq!(check(owner, &a), Err(VerifyError::BadSignature));
    }
}

#[test]
fn signature_from_another_key_is_rejected() {
    let pairs = [
        (Owner::es256(1), Owner::es256(9)),
        (Owner::eddsa(2), Owner::eddsa(9)),
    ];
    for (owner, impostor) in &pairs {
        let mut a = impostor.assertion();
        a.credential_id = encode(&owner.id);
        assert_eq!(check(owner, &a), Err(VerifyError::BadSignature));
    }
}

#[test]
fn signature_of_the_wrong_algorithm_is_malformed() {
    let (es, ed) = (Owner::es256(1), Owner::eddsa(2));
    let mut a = ed.assertion();
    a.credential_id = encode(&es.id);
    assert_eq!(check(&es, &a), Err(VerifyError::Malformed("signature")));
    let mut a = es.assertion();
    a.credential_id = encode(&ed.id);
    assert!(matches!(
        check(&ed, &a),
        Err(VerifyError::Malformed("signature") | VerifyError::BadSignature)
    ));
}

#[test]
fn unknown_credential_is_rejected() {
    let owner = Owner::es256(1);
    let stranger = Owner::es256(5);
    assert_eq!(
        check(&owner, &stranger.assertion()),
        Err(VerifyError::UnknownKey)
    );
    assert_eq!(
        verify(&owner.assertion(), &[], &EXPECTED),
        Err(VerifyError::UnknownKey)
    );
}

#[test]
fn malformed_input_never_panics() {
    let garbage = [
        "",
        "!!!",
        "====",
        "a",
        "ab=c",
        "\u{0}",
        "AAAA",
        "_-_-",
        "eyJ0eXBlIjo",
        "e30",        // "{}"
        "bnVsbA",     // "null"
        "WzEsMiwzXQ", // "[1,2,3]"
    ];
    for owner in &owners() {
        let good = owner.assertion();
        let key = owner.key();
        for g in garbage {
            for field in 0..4 {
                let mut a = good.clone();
                *[
                    &mut a.credential_id,
                    &mut a.authenticator_data,
                    &mut a.client_data_json,
                    &mut a.signature,
                ][field] = g.to_string();
                assert!(verify(&a, std::slice::from_ref(&key), &EXPECTED).is_err());
            }
            let mut bad_key = key.clone();
            bad_key.public_key = g.to_string();
            assert!(verify(&good, &[bad_key], &EXPECTED).is_err());
        }
        // Every truncation of every decoded field.
        for field in 0..4 {
            let raw = {
                let a = &good;
                b64([
                    &a.credential_id,
                    &a.authenticator_data,
                    &a.client_data_json,
                    &a.signature,
                ][field])
            };
            for len in 0..raw.len() {
                let mut a = good.clone();
                *[
                    &mut a.credential_id,
                    &mut a.authenticator_data,
                    &mut a.client_data_json,
                    &mut a.signature,
                ][field] = encode(&raw[..len]);
                assert!(verify(&a, std::slice::from_ref(&key), &EXPECTED).is_err());
            }
        }
        // Validly signed but non-JSON, wrong-shape or duplicate-field client data.
        for cd in [
            &b"\xff\xfe not json"[..],
            b"{}",
            b"[]",
            br#"{"type":1,"challenge":"x","origin":"y"}"#,
            br#"{"type":"webauthn.get","type":"webauthn.get","challenge":"x","origin":"y"}"#,
        ] {
            let a = owner.assert_raw(&auth_data(RP, UP_UV), cd);
            assert_eq!(check(owner, &a), Err(VerifyError::Malformed("client data")));
        }
    }
    // A DER signature that's garbage, and a public key that's not on the curve.
    let es = Owner::es256(1);
    let mut a = es.assertion();
    a.signature = encode(&[0x30, 0x06, 0x02, 0x01, 0x00, 0x02, 0x01, 0x00]);
    assert_eq!(check(&es, &a), Err(VerifyError::Malformed("signature")));
    let mut off_curve = vec![0x04];
    off_curve.extend_from_slice(&[1u8; 64]);
    assert_eq!(
        OwnerKey::from_raw(&es.id, KeyAlg::Es256, &off_curve),
        Err(VerifyError::Malformed("public key"))
    );
    assert!(OwnerKey::from_raw(&es.id, KeyAlg::EdDsa, &[0u8; 31]).is_err());
}

fn cose_map(entries: Vec<(i64, Value)>) -> Vec<u8> {
    let map = Value::Map(
        entries
            .into_iter()
            .map(|(k, v)| (Value::Integer(k.into()), v))
            .collect(),
    );
    let mut out = Vec::new();
    ciborium::into_writer(&map, &mut out).unwrap();
    out
}

fn es256_cose(raw: &[u8]) -> Vec<u8> {
    cose_map(vec![
        (1, Value::Integer(2.into())),
        (3, Value::Integer((-7).into())),
        (-1, Value::Integer(1.into())),
        (-2, Value::Bytes(raw[1..33].to_vec())),
        (-3, Value::Bytes(raw[33..].to_vec())),
    ])
}

#[test]
fn cose_keys_round_trip() {
    for owner in &owners() {
        let key = owner.key();
        let raw = b64(&key.public_key);
        let cose = match key.alg {
            KeyAlg::Es256 => es256_cose(&raw),
            KeyAlg::EdDsa => cose_map(vec![
                (1, Value::Integer(1.into())),
                (3, Value::Integer((-8).into())),
                (-1, Value::Integer(6.into())),
                (-2, Value::Bytes(raw.clone())),
            ]),
        };
        let parsed = owner_key_from_cose(&key.credential_id, &cose).unwrap();
        assert_eq!(parsed, key);
        assert_eq!(verify(&owner.assertion(), &[parsed], &EXPECTED), Ok(()));
    }
}

#[test]
fn cose_keys_that_are_not_es256_or_eddsa_are_rejected() {
    let id = encode(b"id");
    let bad = VerifyError::Malformed("COSE key");
    // RS256: kty 3, alg -257, n, e.
    let rs256 = cose_map(vec![
        (1, Value::Integer(3.into())),
        (3, Value::Integer((-257).into())),
        (-1, Value::Bytes(vec![0xab; 256])),
        (-2, Value::Bytes(vec![1, 0, 1])),
    ]);
    assert_eq!(owner_key_from_cose(&id, &rs256), Err(bad.clone()));

    let raw = b64(&Owner::es256(1).key().public_key);
    let good = es256_cose(&raw);
    assert!(owner_key_from_cose(&id, &good).is_ok());
    // P-384 curve, wrong alg, short coordinate, trailing bytes, duplicate label, not a map.
    let p384 = cose_map(vec![
        (1, Value::Integer(2.into())),
        (3, Value::Integer((-7).into())),
        (-1, Value::Integer(2.into())),
        (-2, Value::Bytes(raw[1..33].to_vec())),
        (-3, Value::Bytes(raw[33..].to_vec())),
    ]);
    let wrong_alg = cose_map(vec![
        (1, Value::Integer(2.into())),
        (3, Value::Integer((-8).into())),
        (-1, Value::Integer(1.into())),
        (-2, Value::Bytes(raw[1..33].to_vec())),
        (-3, Value::Bytes(raw[33..].to_vec())),
    ]);
    let short = cose_map(vec![
        (1, Value::Integer(2.into())),
        (3, Value::Integer((-7).into())),
        (-1, Value::Integer(1.into())),
        (-2, Value::Bytes(raw[1..32].to_vec())),
        (-3, Value::Bytes(raw[33..].to_vec())),
    ]);
    let mut trailing = good.clone();
    trailing.push(0);
    let duplicate = cose_map(vec![
        (1, Value::Integer(2.into())),
        (3, Value::Integer((-7).into())),
        (-1, Value::Integer(1.into())),
        (-2, Value::Bytes(raw[1..33].to_vec())),
        (-3, Value::Bytes(raw[33..].to_vec())),
        (1, Value::Integer(1.into())),
    ]);
    let mut array = Vec::new();
    ciborium::into_writer(&Value::Array(vec![]), &mut array).unwrap();
    for cose in [
        p384,
        wrong_alg,
        short,
        trailing,
        duplicate,
        array,
        vec![],
        vec![0xff],
        good[..10].to_vec(),
    ] {
        assert_eq!(owner_key_from_cose(&id, &cose), Err(bad.clone()));
    }
    assert_eq!(
        owner_key_from_cose("!!", &good),
        Err(VerifyError::Malformed("credential id"))
    );
}

/// A real assertion from a `YubiKey` 5, captured by the webauthn-rs project
/// (webauthn-rs-core 0.6.2, `core.rs`, `test_authentication`): rp id
/// `localhost`, origin `http://localhost:8080`. It was made without user
/// verification, so the full check gets as far as the UV flag; every earlier
/// step, including the signature, has passed by then.
#[test]
fn real_yubikey_assertion() {
    let challenge = [
        90, 5, 243, 254, 68, 239, 221, 101, 20, 214, 76, 60, 134, 111, 142, 26, 129, 146, 225, 144,
        135, 95, 253, 219, 18, 161, 199, 216, 251, 213, 167, 195,
    ];
    let x = [
        46, 121, 76, 233, 118, 208, 250, 74, 227, 182, 8, 145, 45, 46, 5, 9, 199, 186, 84, 83, 7,
        237, 130, 73, 16, 90, 17, 54, 33, 255, 54, 56,
    ];
    let y = [
        117, 105, 1, 23, 253, 223, 67, 135, 253, 219, 253, 223, 17, 247, 91, 197, 205, 225, 143,
        59, 47, 138, 70, 120, 74, 155, 177, 177, 166, 233, 48, 71,
    ];
    let id =
        "at-FfKGsOI21EhtCu7Vx-7t7FKkpUOyKXIkEBBD_vC-eym_AdW6Y9V8WyKxHmii11EBQEe7uFQ0bkYwb0GWmUQ";
    let mut raw = vec![0x04];
    raw.extend_from_slice(&x);
    raw.extend_from_slice(&y);
    let key = owner_key_from_cose(id, &es256_cose(&raw)).unwrap();
    let assertion = Assertion {
        credential_id: id.to_string(),
        authenticator_data: "SZYN5YgOjGh0NBcPZHZgW4_krrmihjLHmVzzuoMdl2MBAAAAFA".to_string(),
        client_data_json: "eyJjaGFsbGVuZ2UiOiJXZ1h6X2tUdjNXVVUxa3c4aG0tT0dvR1M0WkNIWF8zYkVxSEgyUHZWcDhNIiwiY2xpZW50RXh0ZW5zaW9ucyI6e30sImhhc2hBbGdvcml0aG0iOiJTSEEtMjU2Iiwib3JpZ2luIjoiaHR0cDovL2xvY2FsaG9zdDo4MDgwIiwidHlwZSI6IndlYmF1dGhuLmdldCJ9".to_string(),
        signature: "MEYCIQDmLVOqv85cdRup4Fr8Pf9zC4AWO-XKBJqa8xPwYFCCMAIhAOiExLoyes0xipmUmq0BVlqJaCKLn_MFKG9GIDsCGq_-".to_string(),
    };
    let expected = Expected {
        rp_id: "localhost",
        origin: "http://localhost:8080",
        challenge: &challenge,
    };
    let keys = [key];
    assert_eq!(
        verify(&assertion, &keys, &expected),
        Err(VerifyError::NotVerified)
    );

    // The same capture fails on each earlier check when it's changed.
    let other = Expected {
        challenge: b"other",
        ..expected
    };
    assert_eq!(
        verify(&assertion, &keys, &other),
        Err(VerifyError::WrongChallenge)
    );
    let other = Expected {
        origin: "https://localhost:8080",
        ..expected
    };
    assert_eq!(
        verify(&assertion, &keys, &other),
        Err(VerifyError::WrongOrigin)
    );
    let other = Expected {
        rp_id: "example.com",
        ..expected
    };
    assert_eq!(verify(&assertion, &keys, &other), Err(VerifyError::WrongRp));
    let mut tampered = assertion.clone();
    tampered.authenticator_data = "SZYN5YgOjGh0NBcPZHZgW4_krrmihjLHmVzzuoMdl2MFAAAAFA".to_string();
    assert_eq!(
        verify(&tampered, &keys, &expected),
        Err(VerifyError::BadSignature)
    );
}

#[test]
fn owner_key_serde_shape() {
    let key = Owner::eddsa(2).key();
    let json = serde_json::to_value(&key).unwrap();
    assert_eq!(json["alg"], "eddsa");
    assert_eq!(serde_json::to_value(KeyAlg::Es256).unwrap(), "es256");
    assert!(!key.credential_id.contains('='));
    let back: OwnerKey = serde_json::from_value(json).unwrap();
    assert_eq!(back, key);
}
