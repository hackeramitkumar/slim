// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("session not found: {0}")]
    NotFound(String),

    #[error("session already exists: {0}")]
    AlreadyExists(String),

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("MLS error: {0}")]
    MlsError(String),

    #[error("transport error: {0}")]
    TransportError(String),

    #[error("channel closed")]
    ChannelClosed,

    #[error("session cancelled")]
    Cancelled,

    #[error("internal error: {0}")]
    Internal(String),
}
