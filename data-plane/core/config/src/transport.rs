// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Grpc,
    WebSocket,
}

impl Default for Transport {
    fn default() -> Self {
        Transport::Grpc
    }
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Grpc => write!(f, "grpc"),
            Transport::WebSocket => write!(f, "websocket"),
        }
    }
}
