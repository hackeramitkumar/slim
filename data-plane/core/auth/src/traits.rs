// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use serde::de::DeserializeOwned;

use crate::errors::AuthError;

#[cfg_attr(feature = "native", async_trait::async_trait)]
#[cfg_attr(feature = "wasm", async_trait::async_trait(?Send))]
pub trait Verifier {
    fn try_get_claims<Claims>(&self, token: impl Into<String>) -> Result<Claims, AuthError>
    where
        Claims: DeserializeOwned;
}

#[cfg_attr(feature = "native", async_trait::async_trait)]
#[cfg_attr(feature = "wasm", async_trait::async_trait(?Send))]
pub trait TokenProvider {
    fn get_token(&self) -> Result<String, AuthError>;

    fn get_id(&self) -> Result<String, AuthError>;

    fn get_signature_secret_key(&self) -> Result<Vec<u8>, AuthError> {
        Err(AuthError::MlsNotSupported)
    }

    fn get_signature_public_key(&self) -> Result<Vec<u8>, AuthError> {
        Err(AuthError::MlsNotSupported)
    }

    fn rotate_signature_keys(&mut self) -> Result<(), AuthError> {
        Err(AuthError::MlsNotSupported)
    }

    fn set_signature_keys(&mut self, secret: Vec<u8>, public: Vec<u8>) -> Result<(), AuthError> {
        let _ = (secret, public);
        Err(AuthError::MlsNotSupported)
    }
}
