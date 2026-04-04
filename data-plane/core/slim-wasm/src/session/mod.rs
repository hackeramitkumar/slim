// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

//! Full-parity session layer for the WASM client.
//!
//! Supports all session types matching native `agntcy-slim-service`:
//! - **FireAndForget** — basic unreliable pub/sub
//! - **FireAndForgetReliable** — with outbound retries, ACK, timeout→error, sticky sessions
//! - **RequestResponse** — request with timer, reply stops timer
//! - **Streaming** — Sender/Receiver/Bidirectional with ordered delivery, RTX, beacons

pub mod producer_buffer;
pub mod receiver_buffer;
pub mod timer;

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use parking_lot::Mutex;
use rand::Rng;
use slim_datapath::api::proto::pubsub::v1::{Message, SessionHeader, SessionHeaderType, SlimHeader};
use slim_datapath::api::ProtoMessage;
use slim_datapath::messages::utils::SlimHeaderFlags;
use slim_datapath::messages::{Agent, AgentType};
use tokio_with_wasm::alias as tokio;
use tracing::{debug, error, warn};

use producer_buffer::ProducerBuffer;
use receiver_buffer::ReceiverBuffer;
use timer::{Timer, TimerEvent, TimerEventKind, TimerType};

pub type Id = u32;

pub const SESSION_RANGE: std::ops::Range<u32> = 0..(u32::MAX - 1000);
const STREAM_BROADCAST: u32 = 50;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for creating a session, parsed from JS parameters.
#[derive(Clone, Debug)]
pub enum SessionConfig {
    FireAndForget,
    FireAndForgetReliable {
        timeout: Duration,
        max_retries: u32,
        sticky: bool,
    },
    RequestResponse {
        timeout: Duration,
    },
    Streaming {
        direction: StreamingDirection,
        max_retries: u32,
        timeout: Duration,
    },
}

#[derive(Clone, Debug)]
pub enum StreamingDirection {
    Sender,
    Receiver,
    Bidirectional,
}

impl SessionConfig {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionConfig::FireAndForget => "fnf",
            SessionConfig::FireAndForgetReliable { .. } => "fnf-reliable",
            SessionConfig::RequestResponse { .. } => "request-response",
            SessionConfig::Streaming { direction, .. } => match direction {
                StreamingDirection::Sender => "streaming-sender",
                StreamingDirection::Receiver => "streaming-receiver",
                StreamingDirection::Bidirectional => "streaming-bidirectional",
            },
        }
    }

    pub fn from_js_params(
        session_type: Option<&str>,
        timeout_ms: Option<u32>,
        max_retries: Option<u32>,
        sticky: Option<bool>,
        direction: Option<&str>,
    ) -> Result<Self, String> {
        match session_type.unwrap_or("fnf") {
            "fnf" | "fire-and-forget" | "fireAndForget" => Ok(SessionConfig::FireAndForget),
            "reliable" | "fnf-reliable" | "fireAndForgetReliable" => {
                Ok(SessionConfig::FireAndForgetReliable {
                    timeout: Duration::from_millis(timeout_ms.unwrap_or(1000) as u64),
                    max_retries: max_retries.unwrap_or(5),
                    sticky: sticky.unwrap_or(false),
                })
            }
            "request-response" | "requestResponse" | "rr" => {
                Ok(SessionConfig::RequestResponse {
                    timeout: Duration::from_millis(timeout_ms.unwrap_or(5000) as u64),
                })
            }
            "streaming" | "stream" => {
                let dir = match direction.unwrap_or("receiver") {
                    "sender" | "producer" => StreamingDirection::Sender,
                    "receiver" | "consumer" => StreamingDirection::Receiver,
                    "bidirectional" | "pubsub" => StreamingDirection::Bidirectional,
                    s => return Err(format!("unknown streaming direction: {s}")),
                };
                Ok(SessionConfig::Streaming {
                    direction: dir,
                    max_retries: max_retries.unwrap_or(10),
                    timeout: Duration::from_millis(timeout_ms.unwrap_or(1000) as u64),
                })
            }
            s => Err(format!(
                "unknown session type '{s}'. Use: fnf, reliable, request-response, streaming"
            )),
        }
    }
}

/// Events delivered to the JS application layer.
pub enum AppEvent {
    Message(Message),
    Error {
        session_id: u32,
        error: String,
        original: Option<Message>,
    },
    MessageLost {
        session_id: u32,
    },
}

// ---------------------------------------------------------------------------
// Internal session state
// ---------------------------------------------------------------------------

#[derive(Default)]
enum StickyState {
    #[default]
    Uninitialized,
    Discovering,
    Established {
        name: Agent,
        connection: u64,
    },
}

struct ReceiverPeer {
    buffer: ReceiverBuffer,
    rtx_map: HashMap<u32, Message>,
    timers: HashMap<u32, Timer>,
    incoming_conn: u64,
}

struct ProducerState {
    buffer: ProducerBuffer,
    next_id: u32,
    beacon_timer: Option<Timer>,
}

enum SessionState {
    Fnf {
        destination: AgentType,
    },
    FnfReliable {
        destination: AgentType,
        timeout: Duration,
        max_retries: u32,
        pending: HashMap<u32, Message>,
        timers: HashMap<u32, Timer>,
        sticky: StickyState,
        sticky_buffer: VecDeque<Message>,
    },
    RequestResponse {
        destination: AgentType,
        timeout: Duration,
        pending: HashMap<u32, Message>,
        timers: HashMap<u32, Timer>,
    },
    Streaming {
        destination: AgentType,
        producer: Option<ProducerState>,
        receivers: Option<HashMap<Agent, ReceiverPeer>>,
        max_retries: u32,
        timeout: Duration,
    },
}

impl SessionState {
    fn destination(&self) -> &AgentType {
        match self {
            SessionState::Fnf { destination, .. }
            | SessionState::FnfReliable { destination, .. }
            | SessionState::RequestResponse { destination, .. }
            | SessionState::Streaming { destination, .. } => destination,
        }
    }

    fn config_str(&self) -> &'static str {
        match self {
            SessionState::Fnf { .. } => "fnf",
            SessionState::FnfReliable { .. } => "fnf-reliable",
            SessionState::RequestResponse { .. } => "request-response",
            SessionState::Streaming { producer, receivers, .. } => {
                match (producer.is_some(), receivers.is_some()) {
                    (true, true) => "streaming-bidirectional",
                    (true, false) => "streaming-sender",
                    _ => "streaming-receiver",
                }
            }
        }
    }
}

fn new_session_state(config: SessionConfig) -> SessionState {
    match config {
        SessionConfig::FireAndForget => SessionState::Fnf {
            destination: AgentType::default(),
        },
        SessionConfig::FireAndForgetReliable {
            timeout,
            max_retries,
            ..
        } => SessionState::FnfReliable {
            destination: AgentType::default(),
            timeout,
            max_retries,
            pending: HashMap::new(),
            timers: HashMap::new(),
            sticky: StickyState::default(),
            sticky_buffer: VecDeque::new(),
        },
        SessionConfig::RequestResponse { timeout } => SessionState::RequestResponse {
            destination: AgentType::default(),
            timeout,
            pending: HashMap::new(),
            timers: HashMap::new(),
        },
        SessionConfig::Streaming {
            direction,
            max_retries,
            timeout,
        } => {
            let producer = match direction {
                StreamingDirection::Sender | StreamingDirection::Bidirectional => {
                    Some(ProducerState {
                        buffer: ProducerBuffer::with_capacity(500),
                        next_id: 0,
                        beacon_timer: None,
                    })
                }
                _ => None,
            };
            let receivers = match direction {
                StreamingDirection::Receiver | StreamingDirection::Bidirectional => {
                    Some(HashMap::new())
                }
                _ => None,
            };
            SessionState::Streaming {
                destination: AgentType::default(),
                producer,
                receivers,
                max_retries,
                timeout,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SessionLayer
// ---------------------------------------------------------------------------

pub struct SessionLayer {
    source: Agent,
    sessions: Mutex<HashMap<Id, SessionState>>,
    tx_ws: tokio::sync::mpsc::Sender<Message>,
    tx_app: tokio::sync::mpsc::Sender<AppEvent>,
    timer_tx: tokio::sync::mpsc::Sender<TimerEvent>,
}

impl SessionLayer {
    pub fn new(
        source: Agent,
        tx_ws: tokio::sync::mpsc::Sender<Message>,
        tx_app: tokio::sync::mpsc::Sender<AppEvent>,
        timer_tx: tokio::sync::mpsc::Sender<TimerEvent>,
    ) -> Self {
        SessionLayer {
            source,
            sessions: Mutex::new(HashMap::new()),
            tx_ws,
            tx_app,
            timer_tx,
        }
    }

    pub fn source(&self) -> &Agent {
        &self.source
    }

    pub fn create_session(&self, destination: AgentType, config: SessionConfig) -> Id {
        let mut pool = self.sessions.lock();
        let id = loop {
            let candidate: u32 = rand::rng().random_range(SESSION_RANGE);
            if !pool.contains_key(&candidate) {
                break candidate;
            }
        };

        let mut state = new_session_state(config);
        // Set the destination on the state
        match &mut state {
            SessionState::Fnf { destination: d, .. }
            | SessionState::FnfReliable { destination: d, .. }
            | SessionState::RequestResponse { destination: d, .. }
            | SessionState::Streaming { destination: d, .. } => {
                *d = destination;
            }
        }
        pool.insert(id, state);
        id
    }

    pub fn delete_session(&self, id: Id) -> bool {
        self.sessions.lock().remove(&id).is_some()
    }

    pub fn get_session_info(&self, id: Id) -> Option<(AgentType, String)> {
        self.sessions
            .lock()
            .get(&id)
            .map(|s| (s.destination().clone(), s.config_str().to_string()))
    }

    // -------------------------------------------------------------------
    // Outbound: App → Network
    // -------------------------------------------------------------------

    pub async fn send_outbound(
        &self,
        session_id: Id,
        payload: &[u8],
        content_type: &str,
        flags: Option<SlimHeaderFlags>,
    ) -> Result<(), String> {
        let (msg, _timer_info) = {
            let mut pool = self.sessions.lock();
            let state = pool
                .get_mut(&session_id)
                .ok_or_else(|| format!("unknown session {session_id}"))?;

            match state {
                SessionState::Fnf { destination } => {
                    let msg = self.build_publish_msg(
                        destination,
                        None,
                        SessionHeaderType::Fnf,
                        session_id,
                        payload,
                        content_type,
                        flags,
                    )?;
                    (msg, None)
                }
                SessionState::FnfReliable {
                    destination,
                    timeout,
                    max_retries,
                    pending,
                    timers,
                    sticky,
                    sticky_buffer,
                } => {
                    let message_id: u32 = rand::rng().random_range(0..u32::MAX);

                    // Handle sticky session routing
                    let (dest_agent_id, extra_flags) = match sticky {
                        StickyState::Established {
                            name, connection, ..
                        } => (
                            Some(name.agent_id()),
                            Some(SlimHeaderFlags::default().with_forward_to(*connection)),
                        ),
                        StickyState::Discovering => {
                            let msg = self.build_publish_raw(
                                destination,
                                None,
                                SessionHeaderType::FnfReliable,
                                session_id,
                                message_id,
                                payload,
                                content_type,
                                flags,
                            )?;
                            sticky_buffer.push_back(msg);
                            return Ok(());
                        }
                        StickyState::Uninitialized => {
                            // Check if sticky is enabled on this session
                            // (we need to check the config, but we don't store it separately)
                            // For now, just send normally - sticky discovery is triggered
                            // separately via the session config.
                            (None, None)
                        }
                    };

                    let merged_flags = match (flags, extra_flags) {
                        (Some(f), Some(ef)) => Some(f.with_forward_to(
                            ef.forward_to.unwrap_or(0),
                        )),
                        (None, Some(ef)) => Some(ef),
                        (f, None) => f,
                    };

                    let msg = self.build_publish_raw(
                        destination,
                        dest_agent_id,
                        SessionHeaderType::FnfReliable,
                        session_id,
                        message_id,
                        payload,
                        content_type,
                        merged_flags,
                    )?;

                    pending.insert(message_id, msg.clone());
                    let timer_dur = *timeout;
                    let timer_retries = *max_retries;
                    let mut timer = Timer::new(
                        message_id,
                        TimerType::Constant,
                        timer_dur,
                        None,
                        Some(timer_retries),
                    );
                    timer.start(session_id, self.timer_tx.clone());
                    timers.insert(message_id, timer);

                    (msg, None)
                }
                SessionState::RequestResponse {
                    destination,
                    timeout,
                    pending,
                    timers,
                } => {
                    let message_id: u32 = rand::rng().random_range(0..u32::MAX);
                    let msg = self.build_publish_raw(
                        destination,
                        None,
                        SessionHeaderType::Request,
                        session_id,
                        message_id,
                        payload,
                        content_type,
                        flags,
                    )?;

                    pending.insert(message_id, msg.clone());
                    let timer_dur = *timeout;
                    let mut timer =
                        Timer::new(message_id, TimerType::Constant, timer_dur, None, Some(0));
                    timer.start(session_id, self.timer_tx.clone());
                    timers.insert(message_id, timer);

                    (msg, None)
                }
                SessionState::Streaming {
                    destination,
                    producer,
                    receivers,
                    ..
                } => {
                    let is_bidirectional = producer.is_some() && receivers.is_some();
                    let prod = producer
                        .as_mut()
                        .ok_or("session is a streaming receiver, cannot publish")?;
                    let header_type = if is_bidirectional {
                        SessionHeaderType::PubSub
                    } else {
                        SessionHeaderType::Stream
                    };

                    let msg_id = prod.next_id;
                    let msg = self.build_publish_raw(
                        destination,
                        None,
                        header_type,
                        session_id,
                        msg_id,
                        payload,
                        content_type,
                        Some(flags.unwrap_or_default().with_fanout(STREAM_BROADCAST)),
                    )?;

                    prod.buffer.push(msg.clone());
                    prod.next_id += 1;

                    // Reset beacon timer
                    let beacon_type = if is_bidirectional {
                        SessionHeaderType::BeaconPubSub
                    } else {
                        SessionHeaderType::BeaconStream
                    };
                    let _ = beacon_type; // used via timer_info
                    let timer_info_out = Some((session_id, msg_id));

                    if let Some(ref mut beacon) = prod.beacon_timer {
                        beacon.reset(session_id, self.timer_tx.clone());
                    } else {
                        let mut beacon = Timer::new(
                            u32::MAX, // special ID for beacon
                            TimerType::Exponential,
                            Duration::from_millis(1000),
                            Some(Duration::from_secs(30)),
                            None, // runs forever until stopped
                        );
                        beacon.start(session_id, self.timer_tx.clone());
                        prod.beacon_timer = Some(beacon);
                    }

                    (msg, timer_info_out)
                }
            }
        };

        self.tx_ws
            .send(msg)
            .await
            .map_err(|e| format!("send failed: {e}"))?;

        Ok(())
    }

    // -------------------------------------------------------------------
    // Inbound: Network → App
    // -------------------------------------------------------------------

    pub async fn dispatch_inbound(&self, msg: Message) {
        if !msg.is_publish() {
            let _ = self.tx_app.send(AppEvent::Message(msg)).await;
            return;
        }

        let header = match msg.try_get_session_header() {
            Some(h) => h.clone(),
            None => {
                let _ = self.tx_app.send(AppEvent::Message(msg)).await;
                return;
            }
        };

        let session_id = header.session_id;
        let message_id = header.message_id;
        let header_type = SessionHeaderType::try_from(header.header_type)
            .unwrap_or(SessionHeaderType::Unspecified);

        // Collect outputs while holding the lock, send after releasing.
        let (to_send, to_deliver, errors) = {
            let mut pool = self.sessions.lock();

            match pool.get_mut(&session_id) {
                Some(state) => self.process_inbound_for_session(
                    state,
                    session_id,
                    message_id,
                    header_type,
                    &msg,
                ),
                None => {
                    // Auto-create session for inbound messages
                    self.auto_create_and_process(
                        &mut pool,
                        session_id,
                        message_id,
                        header_type,
                        &msg,
                    )
                }
            }
        };

        // Send outbound messages (ACKs, RTXes, etc.)
        for out_msg in to_send {
            if let Err(e) = self.tx_ws.send(out_msg).await {
                warn!("failed to send outbound: {e}");
            }
        }

        // Deliver messages to app
        for opt_msg in to_deliver {
            match opt_msg {
                Some(m) => {
                    let _ = self.tx_app.send(AppEvent::Message(m)).await;
                }
                None => {
                    let _ = self
                        .tx_app
                        .send(AppEvent::MessageLost { session_id })
                        .await;
                }
            }
        }

        // Deliver errors
        for (sid, err, orig) in errors {
            let _ = self
                .tx_app
                .send(AppEvent::Error {
                    session_id: sid,
                    error: err,
                    original: orig,
                })
                .await;
        }
    }

    fn process_inbound_for_session(
        &self,
        state: &mut SessionState,
        session_id: u32,
        message_id: u32,
        header_type: SessionHeaderType,
        msg: &Message,
    ) -> (Vec<Message>, Vec<Option<Message>>, Vec<(u32, String, Option<Message>)>) {
        match state {
            SessionState::Fnf { .. } => {
                (vec![], vec![Some(msg.clone())], vec![])
            }
            SessionState::FnfReliable {
                pending,
                timers,
                sticky,
                sticky_buffer,
                ..
            } => match header_type {
                SessionHeaderType::Fnf | SessionHeaderType::FnfReliable => {
                    let mut outbound = vec![];
                    if header_type == SessionHeaderType::FnfReliable {
                        outbound.push(self.build_ack(msg, session_id, message_id));
                    }
                    (outbound, vec![Some(msg.clone())], vec![])
                }
                SessionHeaderType::FnfAck => {
                    if let Some(mut timer) = timers.remove(&message_id) {
                        timer.stop();
                    }
                    pending.remove(&message_id);
                    debug!("FnfAck received: session={session_id} msg_id={message_id}");
                    (vec![], vec![], vec![])
                }
                SessionHeaderType::FnfDiscovery => {
                    let source = msg.get_source();
                    let incoming_conn = msg.get_incoming_conn();

                    let reply = self.build_sticky_reply(&source, incoming_conn, session_id);

                    *sticky = StickyState::Established {
                        name: source,
                        connection: incoming_conn,
                    };
                    debug!("sticky session established (responder) session={session_id}");
                    (vec![reply], vec![], vec![])
                }
                SessionHeaderType::FnfDiscoveryReply => {
                    let source = msg.get_source();
                    let incoming_conn = msg.get_incoming_conn();

                    match sticky {
                        StickyState::Discovering => {
                            *sticky = StickyState::Established {
                                name: source,
                                connection: incoming_conn,
                            };
                            debug!("sticky session established (initiator) session={session_id}");

                            let mut buffered_msgs: Vec<Message> =
                                sticky_buffer.drain(..).collect();
                            // Re-stamp buffered messages with sticky destination
                            for bm in &mut buffered_msgs {
                                if let StickyState::Established { name, connection } = sticky {
                                    bm.get_slim_header_mut().set_destination(name);
                                    bm.get_slim_header_mut()
                                        .set_forward_to(Some(*connection));
                                }
                            }
                            (buffered_msgs, vec![], vec![])
                        }
                        _ => (vec![], vec![], vec![]),
                    }
                }
                _ => (vec![], vec![Some(msg.clone())], vec![]),
            },
            SessionState::RequestResponse {
                pending, timers, ..
            } => match header_type {
                SessionHeaderType::Reply => {
                    if let Some(mut timer) = timers.remove(&message_id) {
                        timer.stop();
                    }
                    pending.remove(&message_id);
                    (vec![], vec![Some(msg.clone())], vec![])
                }
                SessionHeaderType::Request => {
                    // Inbound request — deliver to app (app will send Reply)
                    (vec![], vec![Some(msg.clone())], vec![])
                }
                _ => (vec![], vec![Some(msg.clone())], vec![]),
            },
            SessionState::Streaming {
                producer,
                receivers,
                max_retries,
                timeout,
                ..
            } => {
                self.process_streaming_inbound(
                    session_id,
                    message_id,
                    header_type,
                    msg,
                    producer,
                    receivers,
                    *max_retries,
                    *timeout,
                )
            }
        }
    }

    fn process_streaming_inbound(
        &self,
        session_id: u32,
        message_id: u32,
        header_type: SessionHeaderType,
        msg: &Message,
        producer: &mut Option<ProducerState>,
        receivers: &mut Option<HashMap<Agent, ReceiverPeer>>,
        max_retries: u32,
        timeout: Duration,
    ) -> (Vec<Message>, Vec<Option<Message>>, Vec<(u32, String, Option<Message>)>) {
        match header_type {
            SessionHeaderType::RtxRequest => {
                // Respond with the requested message from producer buffer
                if let Some(prod) = producer.as_ref() {
                    let source = msg.get_source();
                    let incoming_conn = msg.get_incoming_conn();

                    let rtx_reply = match prod.buffer.get(message_id as usize) {
                        Some(packet) => {
                            let payload = packet
                                .get_payload()
                                .map(|p| p.blob.clone())
                                .unwrap_or_default();

                            ProtoMessage::new_publish_with_headers(
                                Some(SlimHeader::new(
                                    &self.source,
                                    source.agent_type(),
                                    Some(source.agent_id()),
                                    Some(
                                        SlimHeaderFlags::default()
                                            .with_forward_to(incoming_conn)
                                            .with_fanout(1),
                                    ),
                                )),
                                Some(SessionHeader::new(
                                    SessionHeaderType::RtxReply.into(),
                                    session_id,
                                    message_id,
                                )),
                                "",
                                payload,
                            )
                        }
                        None => {
                            let flags = SlimHeaderFlags::default()
                                .with_forward_to(incoming_conn)
                                .with_error(true);
                            ProtoMessage::new_publish_with_headers(
                                Some(SlimHeader::new(
                                    &self.source,
                                    source.agent_type(),
                                    Some(source.agent_id()),
                                    Some(flags),
                                )),
                                Some(SessionHeader::new(
                                    SessionHeaderType::RtxReply.into(),
                                    session_id,
                                    message_id,
                                )),
                                "",
                                vec![],
                            )
                        }
                    };
                    (vec![rtx_reply], vec![], vec![])
                } else {
                    warn!("received RTX request on receiver-only session {session_id}");
                    (vec![], vec![], vec![])
                }
            }

            SessionHeaderType::Stream
            | SessionHeaderType::PubSub
            | SessionHeaderType::RtxReply
            | SessionHeaderType::BeaconStream
            | SessionHeaderType::BeaconPubSub => {
                if let Some(recv_map) = receivers.as_mut() {
                    let producer_name = msg.get_source();
                    let producer_conn = msg.get_incoming_conn();

                    let receiver = recv_map.entry(producer_name.clone()).or_insert_with(|| {
                        ReceiverPeer {
                            buffer: ReceiverBuffer::default(),
                            rtx_map: HashMap::new(),
                            timers: HashMap::new(),
                            incoming_conn: producer_conn,
                        }
                    });
                    receiver.incoming_conn = producer_conn;

                    let mut to_deliver = vec![];
                    let mut to_send = vec![];

                    match header_type {
                        SessionHeaderType::Stream | SessionHeaderType::PubSub => {
                            let (recv, rtx) = receiver.buffer.on_received_message(msg.clone());
                            to_deliver = recv;
                            self.create_rtx_messages(
                                &mut to_send,
                                receiver,
                                &producer_name,
                                producer_conn,
                                session_id,
                                &rtx,
                                max_retries,
                                timeout,
                            );
                        }
                        SessionHeaderType::RtxReply => {
                            if msg.get_error().is_some() && msg.get_error().unwrap() {
                                to_deliver = receiver.buffer.on_lost_message(message_id);
                            } else {
                                let (recv, rtx) =
                                    receiver.buffer.on_received_message(msg.clone());
                                to_deliver = recv;
                                self.create_rtx_messages(
                                    &mut to_send,
                                    receiver,
                                    &producer_name,
                                    producer_conn,
                                    session_id,
                                    &rtx,
                                    max_retries,
                                    timeout,
                                );
                            }
                            // Clean up RTX state
                            if let Some(mut timer) = receiver.timers.remove(&message_id) {
                                timer.stop();
                            }
                            receiver.rtx_map.remove(&message_id);
                        }
                        SessionHeaderType::BeaconStream
                        | SessionHeaderType::BeaconPubSub => {
                            let rtx = receiver.buffer.on_beacon_message(message_id);
                            self.create_rtx_messages(
                                &mut to_send,
                                receiver,
                                &producer_name,
                                producer_conn,
                                session_id,
                                &rtx,
                                max_retries,
                                timeout,
                            );
                        }
                        _ => {}
                    }

                    (to_send, to_deliver, vec![])
                } else {
                    warn!("received stream message on sender-only session {session_id}");
                    (vec![], vec![], vec![])
                }
            }

            _ => {
                debug!("unhandled header type {header_type:?} on streaming session {session_id}");
                (vec![], vec![Some(msg.clone())], vec![])
            }
        }
    }

    fn create_rtx_messages(
        &self,
        to_send: &mut Vec<Message>,
        receiver: &mut ReceiverPeer,
        producer_name: &Agent,
        producer_conn: u64,
        session_id: u32,
        rtx_ids: &[u32],
        max_retries: u32,
        timeout: Duration,
    ) {
        for &r in rtx_ids {
            let rtx_msg = ProtoMessage::new_publish_with_headers(
                Some(SlimHeader::new(
                    &self.source,
                    producer_name.agent_type(),
                    Some(producer_name.agent_id()),
                    Some(SlimHeaderFlags::default().with_forward_to(producer_conn)),
                )),
                Some(SessionHeader::new(
                    SessionHeaderType::RtxRequest.into(),
                    session_id,
                    r,
                )),
                "",
                vec![],
            );

            let mut timer = Timer::new(
                r,
                TimerType::Constant,
                timeout,
                None,
                Some(max_retries),
            );
            timer.start(session_id, self.timer_tx.clone());

            receiver.rtx_map.insert(r, rtx_msg.clone());
            receiver.timers.insert(r, timer);
            to_send.push(rtx_msg);
        }
    }

    fn auto_create_and_process(
        &self,
        pool: &mut HashMap<Id, SessionState>,
        session_id: u32,
        message_id: u32,
        header_type: SessionHeaderType,
        msg: &Message,
    ) -> (Vec<Message>, Vec<Option<Message>>, Vec<(u32, String, Option<Message>)>) {
        let source = msg.get_source();

        let state = match header_type {
            SessionHeaderType::Fnf => SessionState::Fnf {
                destination: source.agent_type().clone(),
            },
            SessionHeaderType::FnfReliable => SessionState::FnfReliable {
                destination: source.agent_type().clone(),
                timeout: Duration::from_millis(1000),
                max_retries: 5,
                pending: HashMap::new(),
                timers: HashMap::new(),
                sticky: StickyState::default(),
                sticky_buffer: VecDeque::new(),
            },
            SessionHeaderType::Request => SessionState::RequestResponse {
                destination: source.agent_type().clone(),
                timeout: Duration::from_millis(5000),
                pending: HashMap::new(),
                timers: HashMap::new(),
            },
            SessionHeaderType::Stream | SessionHeaderType::PubSub => SessionState::Streaming {
                destination: source.agent_type().clone(),
                producer: None,
                receivers: Some(HashMap::new()),
                max_retries: 10,
                timeout: Duration::from_millis(1000),
            },
            _ => {
                debug!("delivering unknown-session message directly: header_type={header_type:?}");
                return (vec![], vec![Some(msg.clone())], vec![]);
            }
        };

        pool.insert(session_id, state);
        debug!("auto-created session {session_id} from inbound {header_type:?}");

        let state = pool.get_mut(&session_id).unwrap();
        self.process_inbound_for_session(state, session_id, message_id, header_type, msg)
    }

    // -------------------------------------------------------------------
    // Timer event handling
    // -------------------------------------------------------------------

    pub async fn handle_timer_event(&self, event: TimerEvent) {
        let session_id = event.session_id;
        let timer_id = event.timer_id;

        match event.kind {
            TimerEventKind::Timeout => {
                self.handle_timer_timeout(session_id, timer_id).await;
            }
            TimerEventKind::Failure => {
                self.handle_timer_failure(session_id, timer_id).await;
            }
        }
    }

    async fn handle_timer_timeout(&self, session_id: u32, timer_id: u32) {
        let msg_to_resend = {
            let pool = self.sessions.lock();
            match pool.get(&session_id) {
                Some(SessionState::FnfReliable { pending, .. }) => {
                    pending.get(&timer_id).cloned()
                }
                Some(SessionState::RequestResponse { pending, .. }) => {
                    // RR has max_retries=0, so timeout goes directly to failure.
                    // This branch shouldn't fire, but handle gracefully.
                    pending.get(&timer_id).cloned()
                }
                Some(SessionState::Streaming {
                    receivers,
                    ..
                }) => {
                    // RTX retransmission
                    if let Some(recv_map) = receivers {
                        for (_, peer) in recv_map.iter() {
                            if let Some(rtx_msg) = peer.rtx_map.get(&timer_id) {
                                return self.send_ws(rtx_msg.clone()).await;
                            }
                        }
                    }
                    // Beacon timer (timer_id == u32::MAX)
                    if timer_id == u32::MAX {
                        if let Some(SessionState::Streaming {
                            producer,
                            destination,
                            ..
                        }) = pool.get(&session_id)
                        {
                            if let Some(prod) = producer {
                                let last_msg_id = prod.next_id.saturating_sub(1);
                                let is_bidir = pool
                                    .get(&session_id)
                                    .map(|s| s.config_str() == "streaming-bidirectional")
                                    .unwrap_or(false);
                                let beacon_type = if is_bidir {
                                    SessionHeaderType::BeaconPubSub
                                } else {
                                    SessionHeaderType::BeaconStream
                                };
                                let beacon = ProtoMessage::new_publish_with_headers(
                                    Some(SlimHeader::new(
                                        &self.source,
                                        destination,
                                        None,
                                        Some(
                                            SlimHeaderFlags::default()
                                                .with_fanout(STREAM_BROADCAST),
                                        ),
                                    )),
                                    Some(SessionHeader::new(
                                        beacon_type.into(),
                                        session_id,
                                        last_msg_id,
                                    )),
                                    "",
                                    vec![],
                                );
                                drop(pool);
                                self.send_ws(beacon).await;
                                return;
                            }
                        }
                    }
                    None
                }
                _ => None,
            }
        };

        if let Some(msg) = msg_to_resend {
            debug!("timer timeout: resending msg_id={timer_id} session={session_id}");
            self.send_ws(msg).await;
        }
    }

    async fn handle_timer_failure(&self, session_id: u32, timer_id: u32) {
        let (error_msg, original) = {
            let mut pool = self.sessions.lock();
            match pool.get_mut(&session_id) {
                Some(SessionState::FnfReliable {
                    pending, timers, ..
                }) => {
                    timers.remove(&timer_id);
                    let orig = pending.remove(&timer_id);
                    (
                        format!("timeout: session={session_id} msg_id={timer_id}"),
                        orig,
                    )
                }
                Some(SessionState::RequestResponse {
                    pending, timers, ..
                }) => {
                    timers.remove(&timer_id);
                    let orig = pending.remove(&timer_id);
                    (
                        format!("request timeout: session={session_id} msg_id={timer_id}"),
                        orig,
                    )
                }
                Some(SessionState::Streaming { receivers, .. }) => {
                    // RTX failure — message is lost
                    if let Some(recv_map) = receivers {
                        for (_, peer) in recv_map.iter_mut() {
                            if peer.timers.contains_key(&timer_id) {
                                peer.timers.remove(&timer_id);
                                peer.rtx_map.remove(&timer_id);
                                let lost_msgs = peer.buffer.on_lost_message(timer_id);
                                drop(pool);

                                for opt in lost_msgs {
                                    match opt {
                                        Some(m) => {
                                            let _ =
                                                self.tx_app.send(AppEvent::Message(m)).await;
                                        }
                                        None => {
                                            let _ = self
                                                .tx_app
                                                .send(AppEvent::MessageLost { session_id })
                                                .await;
                                        }
                                    }
                                }
                                return;
                            }
                        }
                    }
                    return;
                }
                _ => return,
            }
        };

        let _ = self
            .tx_app
            .send(AppEvent::Error {
                session_id,
                error: error_msg,
                original,
            })
            .await;
    }

    // -------------------------------------------------------------------
    // Sticky session support
    // -------------------------------------------------------------------

    pub async fn start_sticky_discovery(
        &self,
        session_id: Id,
    ) -> Result<(), String> {
        let discovery_msg = {
            let mut pool = self.sessions.lock();
            let state = pool
                .get_mut(&session_id)
                .ok_or_else(|| format!("unknown session {session_id}"))?;

            match state {
                SessionState::FnfReliable {
                    destination,
                    sticky,
                    ..
                } => {
                    *sticky = StickyState::Discovering;
                    let msg = ProtoMessage::new_publish_with_headers(
                        Some(SlimHeader::new(&self.source, destination, None, None)),
                        Some(SessionHeader::new(
                            SessionHeaderType::FnfDiscovery.into(),
                            session_id,
                            0,
                        )),
                        "",
                        vec![],
                    );
                    msg
                }
                _ => return Err("sticky discovery only supported on FnfReliable sessions".into()),
            }
        };

        self.send_ws(discovery_msg).await;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Reply support (for RequestResponse inbound requests)
    // -------------------------------------------------------------------

    pub async fn send_reply(
        &self,
        session_id: Id,
        message_id: u32,
        payload: &[u8],
        content_type: &str,
    ) -> Result<(), String> {
        let msg = {
            let pool = self.sessions.lock();
            let state = pool
                .get(&session_id)
                .ok_or_else(|| format!("unknown session {session_id}"))?;

            match state {
                SessionState::RequestResponse { destination, .. } => {
                    self.build_publish_raw(
                        destination,
                        None,
                        SessionHeaderType::Reply,
                        session_id,
                        message_id,
                        payload,
                        content_type,
                        None,
                    )?
                }
                _ => return Err("send_reply only supported on RequestResponse sessions".into()),
            }
        };

        self.send_ws(msg).await;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------

    fn build_publish_msg(
        &self,
        destination: &AgentType,
        dest_agent_id: Option<u64>,
        header_type: SessionHeaderType,
        session_id: u32,
        payload: &[u8],
        content_type: &str,
        flags: Option<SlimHeaderFlags>,
    ) -> Result<Message, String> {
        let message_id: u32 = rand::rng().random_range(0..u32::MAX);
        self.build_publish_raw(
            destination,
            dest_agent_id,
            header_type,
            session_id,
            message_id,
            payload,
            content_type,
            flags,
        )
    }

    fn build_publish_raw(
        &self,
        destination: &AgentType,
        dest_agent_id: Option<u64>,
        header_type: SessionHeaderType,
        session_id: u32,
        message_id: u32,
        payload: &[u8],
        content_type: &str,
        flags: Option<SlimHeaderFlags>,
    ) -> Result<Message, String> {
        let slim_header = SlimHeader::new(
            &self.source,
            destination,
            dest_agent_id,
            Some(flags.unwrap_or_default()),
        );
        let session_header =
            SessionHeader::new(header_type as i32, session_id, message_id);

        let msg = ProtoMessage::new_publish_with_headers(
            Some(slim_header),
            Some(session_header),
            content_type,
            payload.to_vec(),
        );

        msg.validate()
            .map_err(|e| format!("invalid message: {e}"))?;
        Ok(msg)
    }

    fn build_ack(&self, msg: &Message, session_id: u32, message_id: u32) -> Message {
        let source = msg.get_source();
        let incoming_conn = msg.get_incoming_conn();
        let slim_header = SlimHeader::new(
            &self.source,
            source.agent_type(),
            Some(source.agent_id()),
            Some(SlimHeaderFlags::default().with_forward_to(incoming_conn)),
        );
        let session_header = SessionHeader::new(
            SessionHeaderType::FnfAck as i32,
            session_id,
            message_id,
        );
        ProtoMessage::new_publish_with_headers(
            Some(slim_header),
            Some(session_header),
            "",
            vec![],
        )
    }

    fn build_sticky_reply(
        &self,
        remote: &Agent,
        incoming_conn: u64,
        session_id: u32,
    ) -> Message {
        ProtoMessage::new_publish_with_headers(
            Some(SlimHeader::new(
                &self.source,
                remote.agent_type(),
                Some(remote.agent_id()),
                Some(SlimHeaderFlags::default().with_forward_to(incoming_conn)),
            )),
            Some(SessionHeader::new(
                SessionHeaderType::FnfDiscoveryReply.into(),
                session_id,
                0,
            )),
            "",
            vec![],
        )
    }

    async fn send_ws(&self, msg: Message) {
        if let Err(e) = self.tx_ws.send(msg).await {
            error!("failed to send via WebSocket channel: {e}");
        }
    }
}
