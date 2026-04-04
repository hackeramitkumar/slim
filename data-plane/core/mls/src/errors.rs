// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use mls_rs::error::IntoAnyError;
use slim_auth::errors::AuthError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MlsError {
    #[error("i/o error")]
    Io(#[from] std::io::Error),
    #[error("serialization/deserialization error")]
    Serde(#[from] serde_json::Error),

    #[error("mls error")]
    Mls(#[from] mls_rs::error::MlsError),

    #[error("crypto provider error: {0}")]
    CryptoProviderError(String),

    #[error("identity provider error: {0}")]
    IdentityProviderError(#[from] AuthError),

    #[error("requested ciphersuite is unavailable")]
    CiphersuiteUnavailable,
    #[error("mls client not initialized")]
    ClientNotInitialized,
    #[error("mls group does not exist")]
    GroupNotExists,

    #[error("no mls add payload found")]
    NoGroupAddPayload,
    #[error("no mls remove payload found")]
    NoGroupRemovePayload,
    #[error("no welcome message generated")]
    NoWelcomeMessage,
    #[error("unknown payload type")]
    UnknownPayloadType,

    #[error("failed to create storage directory: {0}")]
    StorageIo(std::io::Error),
    #[error("failed to get token: {0}")]
    TokenRetrievalFailed(String),
    #[error("failed to sync file: {0}")]
    FileSyncFailed(String),
    #[error("identifier not found: {0}")]
    IdentifierNotFound(String),
    #[error("credential not found in stored identity")]
    CredentialNotFound,

    #[error("not a basic credential")]
    NotBasicCredential,
    #[error("invalid UTF-8 in credential")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("identity verification failed: {0}")]
    VerificationFailed(String),
    #[error("external sender validation failed: {0}")]
    ExternalSenderFailed(String),
    #[error(
        "public key mismatch: identity public key does not match provided public key: expected: {expected}, found: {found}"
    )]
    PublicKeyMismatch { expected: String, found: String },
    #[error("external commit not supported")]
    ExternalCommitNotSupported,
    #[error("key package credential rejected: {0}")]
    KeyPackageCredentialRejected(String),
    #[error("key package missing from message")]
    KeyPackageMissing,
}

impl IntoAnyError for MlsError {}

impl MlsError {
    pub fn crypto_provider<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        MlsError::CryptoProviderError(e.to_string())
    }

    pub fn token_retrieval_failed<T: std::fmt::Display>(t: T) -> Self {
        MlsError::TokenRetrievalFailed(t.to_string())
    }

    pub fn identifier_not_found<I: std::fmt::Display>(id: I) -> Self {
        MlsError::IdentifierNotFound(id.to_string())
    }

    pub fn verification_failed<R: std::fmt::Display>(reason: R) -> Self {
        MlsError::VerificationFailed(reason.to_string())
    }

    pub fn external_sender_failed<R: std::fmt::Display>(reason: R) -> Self {
        MlsError::ExternalSenderFailed(reason.to_string())
    }
}
