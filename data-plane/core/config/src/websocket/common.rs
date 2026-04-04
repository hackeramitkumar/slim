// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use std::str::FromStr;

use http_body_util::Empty;
use hyper::body::Bytes;
use hyper::header::{CONNECTION, UPGRADE};
use hyper::Request;
use tokio::net::TcpStream;

pub fn extract_query_param(uri: &str, param_name: &str) -> Option<String> {
    let uri_str = uri.to_string();
    let query = uri_str.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if let (Some(key), Some(value)) = (kv.next(), kv.next()) {
            if key == param_name {
                return Some(value.to_string());
            }
        }
    }
    None
}

struct TokioSpawnExecutor;

impl<Fut> hyper::rt::Executor<Fut> for TokioSpawnExecutor
where
    Fut: std::future::Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    fn execute(&self, fut: Fut) {
        tokio::spawn(fut);
    }
}

pub async fn connect_ws(
    endpoint: &str,
) -> Result<fastwebsockets::WebSocket<hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>>, Box<dyn std::error::Error + Send + Sync>>
{
    let uri = hyper::Uri::from_str(endpoint)?;

    let host = uri.host().ok_or("missing host")?;
    let port = uri.port_u16().unwrap_or(match uri.scheme_str() {
        Some("wss") => 443,
        _ => 80,
    });

    let addr = format!("{}:{}", host, port);
    let stream = TcpStream::connect(&addr).await?;

    let req = Request::builder()
        .method("GET")
        .uri(uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/"))
        .header("Host", host)
        .header(UPGRADE, "websocket")
        .header(CONNECTION, "Upgrade")
        .header(
            "Sec-WebSocket-Key",
            fastwebsockets::handshake::generate_key(),
        )
        .header("Sec-WebSocket-Version", "13")
        .body(Empty::<Bytes>::new())?;

    let (ws, _) = fastwebsockets::handshake::client(&TokioSpawnExecutor, req, stream).await?;
    Ok(ws)
}
