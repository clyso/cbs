// CRT core — detached OpenPGP sealing & verification (design §6).
// Copyright (C) 2026 Clyso
// SPDX-License-Identifier: GPL-3.0-or-later

//! Detached OpenPGP signatures over the canonical manifest bytes — the
//! authenticity anchor (design §6). Pure compute over in-memory key bytes:
//! the caller supplies the armored key material (the private key from Vault at
//! sign time, the published public key at verify time) and the RNG, so this
//! crate sources neither keys nor entropy itself.

use pgp::composed::{
    ArmorOptions, Deserializable, DetachedSignature, SignedPublicKey, SignedSecretKey,
};
use pgp::crypto::hash::HashAlgorithm;
use pgp::types::Password;
use rand::{CryptoRng, Rng};

use crate::{ArmoredSignature, CrtCoreError};

/// Produce a detached, ASCII-armored OpenPGP signature over `canonical_bytes`
/// (the RFC 8785 manifest bytes, design §6) using an armored OpenPGP secret
/// key. `rng` is injected so this crate sources no entropy; `key_password` is
/// the key's passphrase if it is protected (Vault keys are typically not).
pub fn sign_manifest<R: Rng + CryptoRng>(
    rng: R,
    canonical_bytes: &[u8],
    secret_key_armored: &str,
    key_password: Option<&str>,
) -> Result<ArmoredSignature, CrtCoreError> {
    let (secret_key, _) = SignedSecretKey::from_string(secret_key_armored)
        .map_err(|e| CrtCoreError::Pgp(format!("parsing secret key: {e}")))?;
    let password = key_password.map_or_else(Password::empty, Password::from);
    let signature = DetachedSignature::sign_binary_data(
        rng,
        &secret_key.primary_key,
        &password,
        HashAlgorithm::Sha256,
        canonical_bytes,
    )
    .map_err(|e| CrtCoreError::Pgp(format!("signing manifest: {e}")))?;
    let armored = signature
        .to_armored_string(ArmorOptions::default())
        .map_err(|e| CrtCoreError::Pgp(format!("armoring signature: {e}")))?;
    Ok(ArmoredSignature(armored))
}

/// Verify a detached signature over `canonical_bytes` against an armored
/// OpenPGP public key (design §6; §11 leg 0). `Ok(())` iff the signature is
/// valid for these exact bytes and this key; any failure (bad bytes, wrong
/// key, malformed signature) is an `Err`.
pub fn verify_manifest(
    canonical_bytes: &[u8],
    signature: &ArmoredSignature,
    public_key_armored: &str,
) -> Result<(), CrtCoreError> {
    let (public_key, _) = SignedPublicKey::from_string(public_key_armored)
        .map_err(|e| CrtCoreError::Pgp(format!("parsing public key: {e}")))?;
    let (detached, _) = DetachedSignature::from_string(&signature.0)
        .map_err(|e| CrtCoreError::Pgp(format!("parsing signature: {e}")))?;
    detached
        .verify(&public_key.primary_key, canonical_bytes)
        .map_err(|e| CrtCoreError::Pgp(format!("signature verification failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pgp::composed::{KeyType, SecretKeyParamsBuilder};

    /// Generate a fresh signing keypair, returned as (armored secret, armored
    /// public). Ed25519 (v4) signing-capable primary, no passphrase.
    fn test_keypair() -> (String, String) {
        let mut params = SecretKeyParamsBuilder::default();
        params
            .key_type(KeyType::Ed25519Legacy)
            .can_certify(true)
            .can_sign(true)
            .primary_user_id("CRT Test <test@example.com>".into())
            .passphrase(None);
        let secret_params = params.build().expect("build key params");
        let secret_key = secret_params
            .generate(rand::thread_rng())
            .expect("generate key");
        let public_key = SignedPublicKey::from(secret_key.clone());
        let secret_armored = secret_key
            .to_armored_string(ArmorOptions::default())
            .expect("armor secret key");
        let public_armored = public_key
            .to_armored_string(ArmorOptions::default())
            .expect("armor public key");
        (secret_armored, public_armored)
    }

    #[test]
    fn sign_then_verify_roundtrips() {
        let (secret, public) = test_keypair();
        let data = b"canonical manifest bytes";
        let sig = sign_manifest(rand::thread_rng(), data, &secret, None).unwrap();
        assert!(sig.0.contains("BEGIN PGP SIGNATURE"), "armored output");
        verify_manifest(data, &sig, &public).expect("valid signature verifies");
    }

    #[test]
    fn tampered_bytes_fail_verification() {
        let (secret, public) = test_keypair();
        let sig = sign_manifest(rand::thread_rng(), b"the original bytes", &secret, None).unwrap();
        // A single different byte must not verify (the integrity/authenticity
        // guarantee, design §6).
        assert!(verify_manifest(b"the original bytez", &sig, &public).is_err());
    }

    #[test]
    fn wrong_key_fails_verification() {
        let (secret, _) = test_keypair();
        let (_, other_public) = test_keypair();
        let data = b"canonical manifest bytes";
        let sig = sign_manifest(rand::thread_rng(), data, &secret, None).unwrap();
        assert!(verify_manifest(data, &sig, &other_public).is_err());
    }

    #[test]
    fn malformed_signature_is_rejected() {
        let (_, public) = test_keypair();
        let bogus = ArmoredSignature("not an armored signature".to_owned());
        assert!(verify_manifest(b"data", &bogus, &public).is_err());
    }
}
