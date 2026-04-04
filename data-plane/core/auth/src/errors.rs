// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),

    #[error("invalid token: {0}")]
    InvalidToken(String),

    #[error("token expired")]
    TokenExpired,

    #[error("missing credentials")]
    MissingCredentials,

    #[error("hmac key is too short")]
    HmacKeyTooShort,

    #[error("hmac key is missing")]
    HmacKeyMissing,

    #[error("no token available")]
    GetTokenError,

    #[error("token invalid")]
    TokenInvalid,

    #[error("token malformed")]
    TokenMalformed,

    #[error("token invalid: missing subject claim")]
    TokenInvalidMissingSub,

    #[error("token invalid: replay")]
    TokenInvalidReplay,

    #[error("token invalid - missing or invalid exp claim")]
    TokenInvalidMissingExp,

    #[error("serialization error: {0}")]
    SerializationError(String),

    #[error("JSON serialization error")]
    JsonError(#[from] serde_json::Error),

    #[error("base64 decode error")]
    Base64DecodeError(#[from] base64::DecodeError),

    #[error("operation would block on async I/O; call async variant")]
    WouldBlockOn,

    #[error("MLS is not supported by this provider")]
    MlsNotSupported,

    #[error("MLS signature key generation failed")]
    MlsKeyGenerationFailed,

    #[error("public key not found in identity claims")]
    PublicKeyNotFound,

    #[error("subject not found in identity claims")]
    SubjectNotFound,

    #[cfg(feature = "native")]
    #[error("jwt error: {0}")]
    JwtError(String),

    #[cfg(feature = "native")]
    #[error("file watcher error: {0}")]
    FileWatcherError(String),

    #[cfg(feature = "native")]
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
}
