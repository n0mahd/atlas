//! Offline check that a release was approved by the owner's passkey.
//!
//! The owner signs a release by doing an ordinary WebAuthn assertion
//! (`navigator.credentials.get`) whose challenge is the release digest. Every
//! machine checks that assertion here against the owner keys it received at
//! pairing, with no server state (WebAuthn Level 2 §7.2, minus the sign
//! counter: replay is handled by release numbers elsewhere).
//!
//! The crate is pure: no I/O, networking or async. Inputs are public, so
//! comparisons need not be constant-time, but malformed input never panics.

use base64::Engine as _;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Base64url without padding on encode; padding is tolerated on decode.
const B64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Authenticator data flag: user present.
const FLAG_UP: u8 = 0x01;
/// Authenticator data flag: user verified.
const FLAG_UV: u8 = 0x04;
/// rpIdHash (32) + flags (1) + signCount (4).
const AUTH_DATA_MIN_LEN: usize = 37;

/// An owner passkey's public key, as distributed to clients at pairing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerKey {
    /// The credential id, base64url without padding.
    pub credential_id: String,
    /// The signature algorithm.
    pub alg: KeyAlg,
    /// Base64url of the raw key: P-256 uncompressed SEC1 (65 bytes) for
    /// [`KeyAlg::Es256`], 32 bytes for [`KeyAlg::EdDsa`].
    pub public_key: String,
}

/// The signature algorithms an owner key may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAlg {
    /// ECDSA over P-256 with SHA-256 (COSE -7).
    Es256,
    /// Ed25519 (COSE -8).
    EdDsa,
}

/// What the browser returned for `navigator.credentials.get`, base64url fields.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Assertion {
    /// `rawId`.
    pub credential_id: String,
    /// `response.authenticatorData`.
    pub authenticator_data: String,
    /// `response.clientDataJSON`.
    pub client_data_json: String,
    /// `response.signature`.
    pub signature: String,
}

/// What the assertion must have been made for.
#[derive(Clone, Copy, Debug)]
pub struct Expected<'a> {
    /// The relying party id, e.g. `atlas.example.com`.
    pub rp_id: &'a str,
    /// The exact origin, e.g. `https://atlas.example.com`.
    pub origin: &'a str,
    /// The raw challenge bytes (the release digest).
    pub challenge: &'a [u8],
}

/// Why an assertion was rejected.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerifyError {
    /// No owner key has the assertion's credential id.
    #[error("the assertion was made by a key that isn't an owner key")]
    UnknownKey,
    /// An input couldn't be decoded or parsed.
    #[error("malformed {0}")]
    Malformed(&'static str),
    /// `clientData.type` isn't `webauthn.get`.
    #[error("the client data isn't for an assertion")]
    WrongType,
    /// `clientData.challenge` isn't the expected challenge.
    #[error("the assertion was made for a different challenge")]
    WrongChallenge,
    /// `clientData.origin` isn't the expected origin, or the call was cross-origin.
    #[error("the assertion was made on a different origin")]
    WrongOrigin,
    /// The authenticator data's rpIdHash isn't SHA-256 of the expected rp id.
    #[error("the assertion was made for a different relying party")]
    WrongRp,
    /// The user-present flag isn't set.
    #[error("the user wasn't present")]
    NotPresent,
    /// The user-verified flag isn't set.
    #[error("the user wasn't verified")]
    NotVerified,
    /// The signature doesn't verify under the owner key.
    #[error("the signature doesn't verify")]
    BadSignature,
}

/// The fields of `CollectedClientData` that the check reads. Duplicate fields
/// are rejected by serde.
#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    type_: String,
    challenge: String,
    origin: String,
    #[serde(rename = "crossOrigin", default)]
    cross_origin: Option<bool>,
}

/// A parsed public key, ready to verify with.
enum Key {
    Es256(p256::ecdsa::VerifyingKey),
    EdDsa(ed25519_dalek::VerifyingKey),
}

impl Key {
    fn parse(alg: KeyAlg, raw: &[u8]) -> Result<Self, VerifyError> {
        const BAD: VerifyError = VerifyError::Malformed("public key");
        match alg {
            KeyAlg::Es256 => {
                if raw.len() != 65 || raw[0] != 0x04 {
                    return Err(BAD);
                }
                p256::ecdsa::VerifyingKey::from_sec1_bytes(raw)
                    .map(Key::Es256)
                    .map_err(|_| BAD)
            }
            KeyAlg::EdDsa => {
                let bytes: &[u8; 32] = raw.try_into().map_err(|_| BAD)?;
                ed25519_dalek::VerifyingKey::from_bytes(bytes)
                    .map(Key::EdDsa)
                    .map_err(|_| BAD)
            }
        }
    }

    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), VerifyError> {
        match self {
            Key::Es256(key) => {
                use p256::ecdsa::signature::Verifier as _;
                let sig = p256::ecdsa::Signature::from_der(signature)
                    .map_err(|_| VerifyError::Malformed("signature"))?;
                key.verify(message, &sig)
                    .map_err(|_| VerifyError::BadSignature)
            }
            Key::EdDsa(key) => {
                let bytes: &[u8; 64] = signature
                    .try_into()
                    .map_err(|_| VerifyError::Malformed("signature"))?;
                let sig = ed25519_dalek::Signature::from_bytes(bytes);
                key.verify_strict(message, &sig)
                    .map_err(|_| VerifyError::BadSignature)
            }
        }
    }
}

impl OwnerKey {
    /// Builds an owner key from a raw credential id and raw public key bytes
    /// (65-byte uncompressed SEC1 for ES256, 32 bytes for Ed25519), checking
    /// that the key is a valid point.
    ///
    /// # Errors
    /// [`VerifyError::Malformed`] if the key isn't valid for `alg`.
    pub fn from_raw(
        credential_id: &[u8],
        alg: KeyAlg,
        public_key: &[u8],
    ) -> Result<Self, VerifyError> {
        Key::parse(alg, public_key)?;
        Ok(OwnerKey {
            credential_id: encode(credential_id),
            alg,
            public_key: encode(public_key),
        })
    }
}

/// Base64url-encodes bytes without padding, as [`OwnerKey`] and
/// `clientData.challenge` expect.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Decodes base64url, with or without padding.
///
/// # Errors
/// [`VerifyError::Malformed`] naming `what` if `s` isn't base64url.
fn decode(s: &str, what: &'static str) -> Result<Vec<u8>, VerifyError> {
    B64.decode(s).map_err(|_| VerifyError::Malformed(what))
}

/// Checks that `assertion` was made by one of the owner `keys`, with the user
/// present and verified, for `expected`'s relying party, origin and challenge.
///
/// The signature is checked before the contents, so a forged assertion is
/// reported as [`VerifyError::BadSignature`] whatever it claims.
///
/// # Errors
/// The first check that fails, as a [`VerifyError`].
pub fn verify(
    assertion: &Assertion,
    keys: &[OwnerKey],
    expected: &Expected,
) -> Result<(), VerifyError> {
    let credential_id = decode(&assertion.credential_id, "credential id")?;
    let owner = keys
        .iter()
        .find(|k| decode(&k.credential_id, "credential id").is_ok_and(|id| id == credential_id))
        .ok_or(VerifyError::UnknownKey)?;
    let key = Key::parse(owner.alg, &decode(&owner.public_key, "public key")?)?;

    let auth_data = decode(&assertion.authenticator_data, "authenticator data")?;
    let client_data_json = decode(&assertion.client_data_json, "client data")?;
    let signature = decode(&assertion.signature, "signature")?;
    if auth_data.len() < AUTH_DATA_MIN_LEN {
        return Err(VerifyError::Malformed("authenticator data"));
    }

    let mut message = Vec::with_capacity(auth_data.len() + 32);
    message.extend_from_slice(&auth_data);
    message.extend_from_slice(&Sha256::digest(&client_data_json));
    key.verify(&message, &signature)?;

    let client: ClientData = serde_json::from_slice(&client_data_json)
        .map_err(|_| VerifyError::Malformed("client data"))?;
    if client.type_ != "webauthn.get" {
        return Err(VerifyError::WrongType);
    }
    if client.challenge != encode(expected.challenge) {
        return Err(VerifyError::WrongChallenge);
    }
    if client.origin != expected.origin || client.cross_origin == Some(true) {
        return Err(VerifyError::WrongOrigin);
    }

    if auth_data[..32] != Sha256::digest(expected.rp_id.as_bytes())[..] {
        return Err(VerifyError::WrongRp);
    }
    let flags = auth_data[32];
    if flags & FLAG_UP == 0 {
        return Err(VerifyError::NotPresent);
    }
    if flags & FLAG_UV == 0 {
        return Err(VerifyError::NotVerified);
    }
    Ok(())
}

/// Converts a `COSE_Key` (CBOR, as stored by webauthn libraries) into an
/// [`OwnerKey`], for ES256 (kty 2, crv 1) and Ed25519 `EdDSA` (kty 1, crv 6). Rejects
/// anything else, including duplicate map keys and trailing bytes.
///
/// `credential_id` is base64url; it's stored without padding.
///
/// # Errors
/// [`VerifyError::Malformed`] if the id or key can't be decoded, or the key
/// isn't a valid ES256 or Ed25519 key.
pub fn owner_key_from_cose(credential_id: &str, cose: &[u8]) -> Result<OwnerKey, VerifyError> {
    use ciborium::Value;
    const BAD: VerifyError = VerifyError::Malformed("COSE key");

    let id = decode(credential_id, "credential id")?;
    let mut reader = cose;
    let value: Value = ciborium::from_reader(&mut reader).map_err(|_| BAD)?;
    if !reader.is_empty() {
        return Err(BAD);
    }
    let Value::Map(entries) = value else {
        return Err(BAD);
    };

    let mut fields: Vec<(i128, Value)> = Vec::with_capacity(entries.len());
    for (label, v) in entries {
        let Value::Integer(label) = label else {
            continue; // Text labels aren't used by the fields read here.
        };
        let label = i128::from(label);
        if fields.iter().any(|(l, _)| *l == label) {
            return Err(BAD);
        }
        fields.push((label, v));
    }
    let int = |label: i128| -> Option<i128> {
        match fields.iter().find(|(l, _)| *l == label)? {
            (_, Value::Integer(i)) => Some(i128::from(*i)),
            _ => None,
        }
    };
    let bytes = |label: i128| -> Option<&[u8]> {
        match fields.iter().find(|(l, _)| *l == label)? {
            (_, Value::Bytes(b)) => Some(b.as_slice()),
            _ => None,
        }
    };

    // COSE labels: 1 kty, 3 alg, -1 crv, -2 x, -3 y.
    let (alg, raw) = match (int(1), int(3), int(-1)) {
        (Some(2), Some(-7), Some(1)) => {
            let (x, y) = (bytes(-2).ok_or(BAD)?, bytes(-3).ok_or(BAD)?);
            if x.len() != 32 || y.len() != 32 {
                return Err(BAD);
            }
            let mut raw = Vec::with_capacity(65);
            raw.push(0x04);
            raw.extend_from_slice(x);
            raw.extend_from_slice(y);
            (KeyAlg::Es256, raw)
        }
        (Some(1), Some(-8), Some(6)) => (KeyAlg::EdDsa, bytes(-2).ok_or(BAD)?.to_vec()),
        _ => return Err(BAD),
    };
    OwnerKey::from_raw(&id, alg, &raw)
}
