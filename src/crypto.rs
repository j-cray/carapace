//! Shared cryptographic helper utilities.

use argon2::{Algorithm, Argon2, Params, Version};
use pbkdf2::pbkdf2_hmac;
use sha2_10::Sha256;
use thiserror::Error;

/// Standard derived-key length for password-based encryption.
pub(crate) const PASSWORD_DERIVED_KEY_LEN: usize = 32;

/// Legacy PBKDF2 iteration count retained for decrypting older persisted
/// formats and for format-v1 compatibility tests.
pub(crate) const LEGACY_PBKDF2_ITERATIONS: u32 = 600_000;

/// Argon2id parameters for current password-derived encryption writes.
pub(crate) const ARGON2ID_V2_MEMORY_KIB: u32 = 64 * 1024;
pub(crate) const ARGON2ID_V2_ITERATIONS: u32 = 3;
pub(crate) const ARGON2ID_V2_LANES: u32 = 1;

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum PasswordKdfError {
    #[error("invalid Argon2 parameters: {0}")]
    InvalidParams(String),
    #[error("Argon2 derivation failed: {0}")]
    DerivationFailed(String),
}

/// Generate a random secret encoded as lowercase hex.
pub(crate) fn generate_hex_secret(byte_len: usize) -> Result<String, getrandom::Error> {
    let mut bytes = vec![0u8; byte_len];
    getrandom::fill(&mut bytes)?;
    Ok(hex::encode(bytes))
}

pub(crate) fn derive_key_pbkdf2_sha256(
    password: &[u8],
    salt: &[u8],
) -> [u8; PASSWORD_DERIVED_KEY_LEN] {
    let mut out = [0u8; PASSWORD_DERIVED_KEY_LEN];
    pbkdf2_hmac::<Sha256>(password, salt, LEGACY_PBKDF2_ITERATIONS, &mut out);
    out
}

pub(crate) fn derive_key_argon2id(
    password: &[u8],
    salt: &[u8],
) -> Result<[u8; PASSWORD_DERIVED_KEY_LEN], PasswordKdfError> {
    let params = Params::new(
        ARGON2ID_V2_MEMORY_KIB,
        ARGON2ID_V2_ITERATIONS,
        ARGON2ID_V2_LANES,
        Some(PASSWORD_DERIVED_KEY_LEN),
    )
    .map_err(|e| PasswordKdfError::InvalidParams(e.to_string()))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; PASSWORD_DERIVED_KEY_LEN];
    argon2
        .hash_password_into(password, salt, &mut out)
        .map_err(|e| PasswordKdfError::DerivationFailed(e.to_string()))?;
    Ok(out)
}
