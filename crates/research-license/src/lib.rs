//! Offline verification of signed licence key files for Gumroad builds.
//!
//! A key file is JSON with two fields. `payload` holds the licence facts.
//! `signature` is an Ed25519 signature over the payload's serialised bytes,
//! base64-encoded. Verification needs only the vendor's public key, which the
//! app supplies. Nothing here touches the network.
//!
//! Signing is not in this crate. The private key stays with the vendor.
//!
//! The open-source build has no key file and no vendor key. [`check`] reports
//! that as [`Status::NotProvided`], which is not an error: the application runs
//! either way. A key file that is present but fails verification is reported as
//! [`Status::Rejected`], so the app can tell the buyer what went wrong.

use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The product this build accepts. Keys for other products are refused.
pub const PRODUCT: &str = "research-os";

/// Errors from reading or verifying a key file.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum LicenseError {
    #[error("licence file is not valid JSON or has the wrong shape: {0}")]
    Malformed(String),
    #[error("signature encoding is invalid")]
    BadSignatureEncoding,
    #[error("public key is not a valid Ed25519 key")]
    BadPublicKey,
    #[error("signature does not match this licence file")]
    InvalidSignature,
    #[error("licence is for product `{0}`, not `{PRODUCT}`")]
    WrongProduct(String),
}

/// The licence facts, signed by the vendor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicensePayload {
    pub product: String,
    /// Unique per purchase, from the vendor's order system.
    pub license_id: String,
    pub licensee: String,
    /// ISO-8601 date the key was issued.
    pub issued: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct KeyFile {
    payload: LicensePayload,
    signature: String,
}

/// A licence that has passed verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct License {
    pub payload: LicensePayload,
}

/// The outcome of checking the licence state of this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// A key file was present and verified.
    Licensed(License),
    /// No key file was supplied. The open-source build is in this state.
    NotProvided,
    /// A key file was supplied but did not verify.
    Rejected(LicenseError),
}

/// Verifies key files against one vendor public key.
pub struct LicenseVerifier {
    key: VerifyingKey,
}

impl LicenseVerifier {
    /// Build a verifier from a public key in text form: the 32 raw key bytes,
    /// base64-encoded.
    pub fn from_base64(text: &str) -> Result<Self, LicenseError> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|_| LicenseError::BadPublicKey)?;
        let key: [u8; 32] = bytes.try_into().map_err(|_| LicenseError::BadPublicKey)?;
        Self::new(&key)
    }

    /// Build a verifier from 32 raw public key bytes.
    pub fn new(public_key: &[u8; 32]) -> Result<Self, LicenseError> {
        VerifyingKey::from_bytes(public_key)
            .map(|key| Self { key })
            .map_err(|_| LicenseError::BadPublicKey)
    }

    /// Verify a key file's signature and product, and return its licence.
    ///
    /// The signature covers the payload re-serialised with `serde_json::to_vec`.
    /// The signer must serialise [`LicensePayload`] the same way, which keeps
    /// field order fixed. Fields the payload does not declare are ignored.
    pub fn verify(&self, key_file_json: &str) -> Result<License, LicenseError> {
        let file: KeyFile = serde_json::from_str(key_file_json)
            .map_err(|e| LicenseError::Malformed(e.to_string()))?;

        let sig_bytes = base64::engine::general_purpose::STANDARD
            .decode(&file.signature)
            .map_err(|_| LicenseError::BadSignatureEncoding)?;
        let signature =
            Signature::from_slice(&sig_bytes).map_err(|_| LicenseError::BadSignatureEncoding)?;

        let payload_bytes = serde_json::to_vec(&file.payload)
            .map_err(|e| LicenseError::Malformed(e.to_string()))?;
        self.key
            .verify(&payload_bytes, &signature)
            .map_err(|_| LicenseError::InvalidSignature)?;

        if file.payload.product != PRODUCT {
            return Err(LicenseError::WrongProduct(file.payload.product));
        }
        Ok(License {
            payload: file.payload,
        })
    }
}

/// Check the licence state of this build.
///
/// `key_file` is the key file's text, or `None` if the user has not supplied one.
pub fn check(verifier: &LicenseVerifier, key_file: Option<&str>) -> Status {
    match key_file {
        None => Status::NotProvided,
        Some(text) => match verifier.verify(text) {
            Ok(license) => Status::Licensed(license),
            Err(e) => Status::Rejected(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn payload() -> LicensePayload {
        LicensePayload {
            product: PRODUCT.into(),
            license_id: "GR-0001".into(),
            licensee: "Test Buyer".into(),
            issued: "2026-10-10".into(),
        }
    }

    /// Test-only issuing. The shipped crate has no signing path.
    fn issue(payload: &LicensePayload, key: &SigningKey) -> String {
        let bytes = serde_json::to_vec(payload).unwrap();
        let signature =
            base64::engine::general_purpose::STANDARD.encode(key.sign(&bytes).to_bytes());
        serde_json::to_string(&KeyFile {
            payload: payload.clone(),
            signature,
        })
        .unwrap()
    }

    fn verifier() -> LicenseVerifier {
        LicenseVerifier::new(&signing_key().verifying_key().to_bytes()).unwrap()
    }

    #[test]
    fn accepts_a_correctly_signed_key() {
        let key_file = issue(&payload(), &signing_key());
        let license = verifier().verify(&key_file).unwrap();
        assert_eq!(license.payload, payload());
    }

    #[test]
    fn rejects_an_edited_payload() {
        let key_file = issue(&payload(), &signing_key());
        let edited = key_file.replace("Test Buyer", "Someone Else");
        assert_eq!(
            verifier().verify(&edited),
            Err(LicenseError::InvalidSignature)
        );
    }

    #[test]
    fn rejects_a_key_from_another_signer() {
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let key_file = issue(&payload(), &other);
        assert_eq!(
            verifier().verify(&key_file),
            Err(LicenseError::InvalidSignature)
        );
    }

    #[test]
    fn rejects_a_key_for_another_product() {
        let mut other_product = payload();
        other_product.product = "twin".into();
        let key_file = issue(&other_product, &signing_key());
        assert_eq!(
            verifier().verify(&key_file),
            Err(LicenseError::WrongProduct("twin".into()))
        );
    }

    #[test]
    fn rejects_malformed_files_and_bad_signature_encodings() {
        assert!(matches!(
            verifier().verify("not json"),
            Err(LicenseError::Malformed(_))
        ));
        let bad_encoding = serde_json::json!({
            "payload": payload(),
            "signature": "!!not base64!!",
        })
        .to_string();
        assert_eq!(
            verifier().verify(&bad_encoding),
            Err(LicenseError::BadSignatureEncoding)
        );
    }

    #[test]
    fn public_key_must_be_32_bytes() {
        let short = base64::engine::general_purpose::STANDARD.encode([1u8; 16]);
        assert_eq!(
            LicenseVerifier::from_base64(&short).err(),
            Some(LicenseError::BadPublicKey)
        );
        let good = base64::engine::general_purpose::STANDARD
            .encode(signing_key().verifying_key().to_bytes());
        assert!(LicenseVerifier::from_base64(&good).is_ok());
    }

    #[test]
    fn open_source_build_without_a_key_is_not_an_error() {
        assert_eq!(check(&verifier(), None), Status::NotProvided);
    }

    #[test]
    fn check_reports_licensed_and_rejected_keys() {
        let good = issue(&payload(), &signing_key());
        assert!(matches!(
            check(&verifier(), Some(&good)),
            Status::Licensed(_)
        ));

        let garbage = "{}";
        assert!(matches!(
            check(&verifier(), Some(garbage)),
            Status::Rejected(LicenseError::Malformed(_))
        ));
    }
}
