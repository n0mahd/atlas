//! A software passkey for tests: answers the hub's WebAuthn options the way a browser and
//! authenticator do (`none` attestation, user present and verified), so tests can enrol
//! and sign in through the real routes.

use p256::ecdsa::SigningKey;
use p256::ecdsa::signature::Signer as _;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use atlas_signer::b64url;

/// The kind of key an authenticator makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Es256,
    /// RSA (COSE RS256): enrols with WebAuthn, but can't sign updates.
    Rs256,
}

#[derive(Debug)]
pub struct Authenticator {
    key: SigningKey,
    pub cred_id: Vec<u8>,
    counter: u32,
    kind: Kind,
    origin: String,
}

/// A minimal CBOR writer: unsigned and negative integers, bytes, text and maps.
fn head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | u8::try_from(n).unwrap()),
        24..=0xff => out.extend([m | 0x18, u8::try_from(n).unwrap()]),
        0x100..=0xffff => {
            out.push(m | 0x19);
            out.extend(u16::try_from(n).unwrap().to_be_bytes());
        }
        _ => {
            out.push(m | 0x1a);
            out.extend(u32::try_from(n).unwrap().to_be_bytes());
        }
    }
}

fn int(out: &mut Vec<u8>, n: i64) {
    if n >= 0 {
        head(out, 0, n.unsigned_abs());
    } else {
        head(out, 1, (-1 - n).unsigned_abs());
    }
}

fn bytes(out: &mut Vec<u8>, b: &[u8]) {
    head(out, 2, b.len() as u64);
    out.extend_from_slice(b);
}

fn text(out: &mut Vec<u8>, t: &str) {
    head(out, 3, t.len() as u64);
    out.extend_from_slice(t.as_bytes());
}

fn challenge_of(options: &Value) -> Vec<u8> {
    let c = options["publicKey"]["challenge"].as_str().unwrap();
    crate::distribute::decode_b64(c).unwrap()
}

fn rp_id(origin: &str) -> String {
    atlas_signer::Url::parse(origin)
        .unwrap()
        .host_str()
        .unwrap()
        .to_owned()
}

impl Authenticator {
    pub fn new(seed: u8, kind: Kind, origin: &str) -> Self {
        Self {
            key: SigningKey::from_slice(&[seed; 32]).unwrap(),
            cred_id: vec![seed; 16],
            counter: 0,
            kind,
            origin: origin.to_owned(),
        }
    }

    fn client_data(&self, kind: &str, challenge: &[u8]) -> String {
        format!(
            r#"{{"type":"{kind}","challenge":"{}","origin":"{}","crossOrigin":false}}"#,
            b64url(challenge),
            self.origin
        )
    }

    fn cose_key(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self.kind {
            Kind::Es256 => {
                let point = self.key.verifying_key().to_sec1_point(false);
                let raw = point.as_bytes();
                head(&mut out, 5, 5);
                int(&mut out, 1);
                int(&mut out, 2);
                int(&mut out, 3);
                int(&mut out, -7);
                int(&mut out, -1);
                int(&mut out, 1);
                int(&mut out, -2);
                bytes(&mut out, &raw[1..33]);
                int(&mut out, -3);
                bytes(&mut out, &raw[33..65]);
            }
            Kind::Rs256 => {
                let mut n = vec![0xc5u8; 256];
                n[255] = 0x01;
                head(&mut out, 5, 4);
                int(&mut out, 1);
                int(&mut out, 3);
                int(&mut out, 3);
                int(&mut out, -257);
                int(&mut out, -1);
                bytes(&mut out, &n);
                int(&mut out, -2);
                bytes(&mut out, &[1, 0, 1]);
            }
        }
        out
    }

    /// Answers `navigator.credentials.create` for the hub's enrolment options.
    pub fn register(&mut self, options: &Value) -> Value {
        let client = self.client_data("webauthn.create", &challenge_of(options));
        let mut auth = Sha256::digest(rp_id(&self.origin).as_bytes()).to_vec();
        auth.push(0x45); // user present, user verified, attested credential data
        auth.extend(self.counter.to_be_bytes());
        auth.extend([0u8; 16]);
        auth.extend(u16::try_from(self.cred_id.len()).unwrap().to_be_bytes());
        auth.extend(&self.cred_id);
        auth.extend(self.cose_key());
        let mut att = Vec::new();
        head(&mut att, 5, 3);
        text(&mut att, "fmt");
        text(&mut att, "none");
        text(&mut att, "attStmt");
        head(&mut att, 5, 0);
        text(&mut att, "authData");
        bytes(&mut att, &auth);
        json!({
            "id": b64url(&self.cred_id),
            "rawId": b64url(&self.cred_id),
            "type": "public-key",
            "extensions": {},
            "response": {
                "attestationObject": b64url(&att),
                "clientDataJSON": b64url(client.as_bytes()),
            }
        })
    }

    /// Answers `navigator.credentials.get` for the hub's sign-in options.
    pub fn assert(&mut self, options: &Value) -> Value {
        self.counter += 1;
        let client = self.client_data("webauthn.get", &challenge_of(options));
        let mut auth = Sha256::digest(rp_id(&self.origin).as_bytes()).to_vec();
        auth.push(0x05); // user present, user verified
        auth.extend(self.counter.to_be_bytes());
        let mut message = auth.clone();
        message.extend_from_slice(&Sha256::digest(client.as_bytes()));
        let sig: p256::ecdsa::Signature = self.key.sign(&message);
        json!({
            "id": b64url(&self.cred_id),
            "rawId": b64url(&self.cred_id),
            "type": "public-key",
            "extensions": {},
            "response": {
                "authenticatorData": b64url(&auth),
                "clientDataJSON": b64url(client.as_bytes()),
                "signature": b64url(sig.to_der().as_bytes()),
                "userHandle": null,
            }
        })
    }

    /// The credential id as the hub shows it.
    pub fn id(&self) -> String {
        b64url(&self.cred_id)
    }
}
