// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

pub mod api;
pub mod messages;
pub mod tables;

mod errors;

#[cfg(feature = "native")]
pub mod message_processing;
#[cfg(feature = "native")]
mod connection;
#[cfg(feature = "native")]
mod forwarder;
#[cfg(feature = "native")]
pub mod websocket;

#[cfg(feature = "native")]
pub use tonic::Status;

#[cfg(not(feature = "native"))]
#[derive(Debug, Clone)]
pub struct Status {
    code: u32,
    message: String,
}

#[cfg(not(feature = "native"))]
impl Status {
    pub fn new(code: u32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(13, message)
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(3, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(5, message)
    }

    pub fn code(&self) -> u32 {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[cfg(not(feature = "native"))]
impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "status: code={}, message={}", self.code, self.message)
    }
}

#[cfg(not(feature = "native"))]
impl std::error::Error for Status {}
