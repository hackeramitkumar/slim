// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;

use crate::transport::Transport;

#[derive(Debug, Clone, Deserialize)]
pub struct WebSocketClientConfig {
    pub endpoint: String,

    #[serde(default)]
    pub transport: Transport,

    pub websocket_auth_query_param: Option<String>,
}

impl Default for WebSocketClientConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            transport: Transport::WebSocket,
            websocket_auth_query_param: None,
        }
    }
}
