// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;

use crate::tls::client::TlsClientConfig;
use crate::transport::Transport;

#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketClientConfig {
    pub endpoint: String,

    #[serde(default)]
    pub transport: Transport,

    #[serde(default, rename = "tls")]
    pub tls_setting: TlsClientConfig,

    /// Name of the query parameter to carry the auth token (e.g. "token").
    pub websocket_auth_query_param: Option<String>,

    /// Shared secret key for HMAC-based token generation.
    /// When set together with `websocket_auth_query_param`, the client will
    /// generate and append a signed token to the connection URL.
    pub shared_secret: Option<String>,

    /// Identity string in `org/ns/app` format used for token claims.
    pub identity: Option<String>,
}

impl Default for WebSocketClientConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            transport: Transport::WebSocket,
            tls_setting: TlsClientConfig::default(),
            websocket_auth_query_param: None,
            shared_secret: None,
            identity: None,
        }
    }
}
