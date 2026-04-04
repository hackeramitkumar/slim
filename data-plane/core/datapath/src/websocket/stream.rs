// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use fastwebsockets::{Frame, OpCode, WebSocket};
use prost::Message as ProstMessage;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::api::proto::pubsub::v1::Message;

pub struct WebSocketStreams {
    pub inbound: ReceiverStream<Message>,
    pub outbound: mpsc::Sender<Message>,
}

pub fn spawn_transport_tasks<S>(
    mut ws: WebSocket<S>,
    cancel: CancellationToken,
) -> WebSocketStreams
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (inbound_tx, inbound_rx) = mpsc::channel::<Message>(256);
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Message>(256);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    debug!("WebSocket task cancelled");
                    break;
                }
                frame_result = ws.read_frame() => {
                    match frame_result {
                        Ok(frame) => match frame.opcode {
                            OpCode::Binary => {
                                match Message::decode(frame.payload.as_ref()) {
                                    Ok(msg) => {
                                        if inbound_tx.send(msg).await.is_err() {
                                            debug!("inbound channel closed");
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        warn!("failed to decode protobuf message: {}", e);
                                    }
                                }
                            }
                            OpCode::Close => {
                                debug!("WebSocket connection closed by remote");
                                break;
                            }
                            _ => {}
                        },
                        Err(e) => {
                            error!("WebSocket read error: {}", e);
                            break;
                        }
                    }
                }
                msg = outbound_rx.recv() => {
                    match msg {
                        Some(msg) => {
                            let bytes = msg.encode_to_vec();
                            let frame = Frame::binary(bytes.into());
                            if let Err(e) = ws.write_frame(frame).await {
                                error!("WebSocket write error: {}", e);
                                break;
                            }
                        }
                        None => {
                            debug!("outbound channel closed");
                            break;
                        }
                    }
                }
            }
        }
    });

    WebSocketStreams {
        inbound: ReceiverStream::new(inbound_rx),
        outbound: outbound_tx,
    }
}
