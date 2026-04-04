// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use crate::errors::SessionError;
use crate::runtime::tokio;
use slim_datapath::api::proto::pubsub::v1::Message;

/// Platform-agnostic session trait.
/// `tokio_with_wasm` handles Send/Sync bounds transparently across platforms.
#[async_trait::async_trait]
pub trait SessionController {
    async fn publish(&self, topic: &str, data: &[u8]) -> Result<(), SessionError>;
    async fn subscribe(&self, topic: &str) -> Result<tokio::sync::mpsc::Receiver<Message>, SessionError>;
    async fn unsubscribe(&self, topic: &str) -> Result<(), SessionError>;
    async fn close(&self) -> Result<(), SessionError>;
}
