// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

#[cfg(feature = "native")]
use mls_rs_core::crypto::CipherSuiteProvider;
#[cfg(feature = "native")]
use mls_rs_core::crypto::CryptoProvider;
#[cfg(feature = "native")]
use mls_rs_crypto_awslc::AwsLcCryptoProvider;

#[cfg(feature = "native")]
const CIPHERSUITE: mls_rs_core::crypto::CipherSuite =
    mls_rs_core::crypto::CipherSuite::CURVE25519_AES128;

#[cfg(feature = "native")]
pub fn generate_mls_signature_keys() -> Result<(Vec<u8>, Vec<u8>), crate::errors::AuthError> {
    let crypto_provider = AwsLcCryptoProvider::default();
    let cipher_suite_provider = crypto_provider
        .cipher_suite_provider(CIPHERSUITE)
        .ok_or(crate::errors::AuthError::MlsKeyGenerationFailed)?;

    let (secret_key, public_key) = cipher_suite_provider
        .signature_key_generate()
        .map_err(|_| crate::errors::AuthError::MlsKeyGenerationFailed)?;

    Ok((
        secret_key.as_bytes().to_vec(),
        public_key.as_bytes().to_vec(),
    ))
}

#[cfg(all(feature = "wasm", not(feature = "native")))]
pub fn generate_mls_signature_keys() -> Result<(Vec<u8>, Vec<u8>), crate::errors::AuthError> {
    Err(crate::errors::AuthError::MlsNotSupported)
}
