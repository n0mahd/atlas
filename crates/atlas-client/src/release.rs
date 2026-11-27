//! Deciding whether a release from the hub may be applied.

use std::fmt::Write as _;

use atlas_core::release::{Manifest, OwnerKey, SignedRelease};
use atlas_verify::{Assertion, Expected, KeyAlg};
use base64::Engine as _;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use sha2::{Digest as _, Sha256};

use crate::config::State;

const B64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// A release that passed every check.
#[derive(Clone, Debug)]
pub struct Checked {
    pub manifest: Manifest,
    /// SHA-256 of the manifest bytes, lowercase hex.
    pub digest: String,
    /// False when this is the release already applied, being applied again.
    pub newer: bool,
}

/// The hub's origin (`scheme://host[:port]`) and its passkey relying-party id (the
/// host).
///
/// # Errors
/// Unless `hub` is an `https://` or `http://` URL with a host.
pub fn origin_and_rp(hub: &str) -> Result<(String, String), String> {
    let bad = || format!("{hub:?} isn't a hub address like https://atlas.example.ts.net");
    let (scheme, rest) = hub.split_once("://").ok_or_else(bad)?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(bad());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(bad());
    }
    let authority = authority.to_ascii_lowercase();
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default().to_owned()
    } else {
        authority.split(':').next().unwrap_or_default().to_owned()
    };
    if host.is_empty() {
        return Err(bad());
    }
    Ok((format!("{scheme}://{authority}"), host))
}

fn verify_key(k: &OwnerKey) -> Option<atlas_verify::OwnerKey> {
    let alg = match k.alg.as_str() {
        "es256" => KeyAlg::Es256,
        "eddsa" => KeyAlg::EdDsa,
        _ => return None,
    };
    Some(atlas_verify::OwnerKey {
        credential_id: k.credential_id.clone(),
        alg,
        public_key: k.public_key.clone(),
    })
}

/// Checks a release against what this login trusts.
///
/// The manifest's exact bytes must carry a passkey signature by a trusted key for this
/// hub, and its number must not go below the last applied one. The release already
/// applied (same number, same bytes) passes without a signature check, so it can be
/// applied again even if it rotated away the key that signed it.
///
/// # Errors
/// Why the release is refused, in words.
pub fn check(signed: &SignedRelease, state: &State, hub: &str) -> Result<Checked, String> {
    let bytes = B64
        .decode(&signed.manifest)
        .map_err(|_| "its manifest isn't base64url".to_owned())?;
    let digest = Sha256::digest(&bytes);
    let digest_hex: String = digest.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    });
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("its manifest isn't valid: {e}"))?;
    if manifest.number != signed.number {
        return Err(format!(
            "it is labelled {} but its manifest says {}",
            signed.number, manifest.number
        ));
    }

    let same_as_applied = state.last_applied == Some(manifest.number)
        && state.last_digest.as_deref() == Some(digest_hex.as_str());
    if !same_as_applied {
        let (origin, rp_id) = origin_and_rp(hub)?;
        let keys: Vec<_> = state.owner_keys.iter().filter_map(verify_key).collect();
        let assertion = Assertion {
            credential_id: signed.signature.credential_id.clone(),
            authenticator_data: signed.signature.authenticator_data.clone(),
            client_data_json: signed.signature.client_data_json.clone(),
            signature: signed.signature.signature.clone(),
        };
        let expected = Expected {
            rp_id: &rp_id,
            origin: &origin,
            challenge: &digest,
        };
        atlas_verify::verify(&assertion, &keys, &expected)
            .map_err(|e| format!("its signature was refused: {e}"))?;
    }

    let newer = match state.last_applied {
        Some(applied) if manifest.number < applied => {
            return Err(format!(
                "it is older than release {applied}, which is already applied (no rollbacks)"
            ));
        }
        Some(applied) if manifest.number == applied => {
            if !same_as_applied {
                return Err(format!(
                    "it reuses the number {applied} with different contents"
                ));
            }
            false
        }
        _ => true,
    };
    let last = if newer { state.last_applied } else { None };
    manifest.check(hub, last).map_err(|e| e.to_string())?;
    Ok(Checked {
        manifest,
        digest: digest_hex,
        newer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(
            origin_and_rp("https://Atlas.example.ts.net/").unwrap(),
            (
                "https://atlas.example.ts.net".to_owned(),
                "atlas.example.ts.net".to_owned()
            )
        );
        assert_eq!(
            origin_and_rp("https://h.example:8443").unwrap(),
            ("https://h.example:8443".to_owned(), "h.example".to_owned())
        );
        for bad in ["atlas.example", "ftp://x", "https://", "https://u@h"] {
            assert!(origin_and_rp(bad).is_err(), "{bad}");
        }
    }
}
