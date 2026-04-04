// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;

use crate::tls::server::TlsServerConfig;
use crate::transport::Transport;

#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketServerConfig {
    pub endpoint: String,

    #[serde(default)]
    pub transport: Transport,

    #[serde(default, rename = "tls")]
    pub tls_setting: TlsServerConfig,

    /// Name of the query parameter that carries the auth token (e.g. "token").
    pub websocket_auth_query_param: Option<String>,

    /// Shared secret key for HMAC-based token verification.
    /// When set together with `websocket_auth_query_param`, the server will
    /// extract the token from the query string and verify it before upgrade.
    pub shared_secret: Option<String>,

    pub max_connections: Option<usize>,
}

impl Default for WebSocketServerConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            transport: Transport::WebSocket,
            tls_setting: TlsServerConfig::default(),
            websocket_auth_query_param: None,
            shared_secret: None,
            max_connections: None,
        }
    }
}
