// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

mod session;

use slim_tracing::TracingConfiguration;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = "initTracing")]
pub fn init_tracing() {
    console_error_panic_hook::set_once();
    let config = TracingConfiguration::default();
    let _ = config.setup_tracing_subscriber();
}

#[cfg(target_arch = "wasm32")]
pub use wasm_impl::GroupChat;

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use std::sync::Arc;

    use parking_lot::Mutex;
    use prost::Message as ProstMessage;
    use slim_auth::shared_secret::SharedSecret;
    use slim_auth::traits::TokenProvider;
    use slim_datapath::api::proto::pubsub::v1::{self as proto, Message, SessionHeaderType};
    use slim_datapath::api::ProtoMessage;
    use slim_datapath::messages::utils::SlimHeaderFlags;
    use slim_datapath::messages::{Agent, AgentType};
    use slim_mls::mls::Mls;
    use futures::StreamExt;
    use tokio_with_wasm::alias as tokio;
    use tracing::{debug, error, info, warn};
    use url::Url;
    use wasm_bindgen::prelude::*;

    use crate::session::{AppEvent, SessionConfig, SessionLayer};
    use crate::session::timer::TimerEvent;

    /// Wraps a JS `Function` so it can be stored inside `Arc<Mutex<...>>`.
    ///
    /// # Safety
    /// This is sound because WASM executes on a single thread in the browser.
    struct JsCallback(js_sys::Function);
    unsafe impl Send for JsCallback {}
    unsafe impl Sync for JsCallback {}

    impl JsCallback {
        fn call1(&self, arg: &JsValue) -> Result<JsValue, JsValue> {
            self.0.call1(&JsValue::NULL, arg)
        }
    }

    #[wasm_bindgen]
    pub struct SlimClient {
        session_layer: Arc<SessionLayer>,
        org: String,
        ns: String,
        app: String,
        tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<Message>>>>,
        on_message_cb: Arc<Mutex<Option<JsCallback>>>,
        connected: Arc<Mutex<bool>>,
    }

    impl SlimClient {
        async fn send_msg(&self, msg: Message) -> Result<(), JsValue> {
            let tx = {
                let guard = self.tx.lock();
                guard
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| JsValue::from_str("disconnected"))?
            };
            tx.send(msg)
                .await
                .map_err(|e| JsValue::from_str(&format!("send failed: {e}")))
        }
    }

    #[wasm_bindgen]
    impl SlimClient {
        #[wasm_bindgen]
        pub async fn connect(
            endpoint: &str,
            shared_secret: &str,
            org: &str,
            ns: &str,
            app: &str,
        ) -> Result<SlimClient, JsValue> {
            let mut url = Url::parse(endpoint)
                .map_err(|e| JsValue::from_str(&format!("invalid url: {e}")))?;

            let app_id = format!("{}/{}/{}", org, ns, app);
            let secret = SharedSecret::new(&app_id, shared_secret)
                .map_err(|e| JsValue::from_str(&format!("auth init failed: {e}")))?;
            let token = secret
                .get_token()
                .map_err(|e| JsValue::from_str(&format!("auth failed: {e}")))?;

            url.query_pairs_mut().append_pair("token", &token);

            let ws = gloo_net::websocket::futures::WebSocket::open(url.as_str())
                .map_err(|e| JsValue::from_str(&format!("ws connect failed: {e}")))?;

            let (sink, mut stream) = ws.split();

            let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(256);
            let on_message_cb: Arc<Mutex<Option<JsCallback>>> = Arc::new(Mutex::new(None));
            let connected: Arc<Mutex<bool>> = Arc::new(Mutex::new(true));

            type WsSink = futures::stream::SplitSink<
                gloo_net::websocket::futures::WebSocket,
                gloo_net::websocket::Message,
            >;
            let write_sink: Arc<tokio::sync::Mutex<WsSink>> =
                Arc::new(tokio::sync::Mutex::new(sink));

            // WebSocket write loop
            let write_handle = write_sink.clone();
            let conn_flag_write = connected.clone();
            tokio_with_wasm::spawn(async move {
                use futures::SinkExt;
                while let Some(msg) = rx.recv().await {
                    let bytes = msg.encode_to_vec();
                    let mut s = write_handle.lock().await;
                    if let Err(e) = s
                        .send(gloo_net::websocket::Message::Bytes(bytes))
                        .await
                    {
                        error!("ws write error: {e}");
                        *conn_flag_write.lock() = false;
                        break;
                    }
                }
                debug!("write loop exited");
            });

            let source = Agent::from_strings(org, ns, app, 0);

            // Create channels for session layer
            let (timer_tx, mut timer_rx) = tokio::sync::mpsc::channel::<TimerEvent>(256);
            let (app_tx, mut app_rx) = tokio::sync::mpsc::channel::<AppEvent>(256);

            let session_layer = Arc::new(SessionLayer::new(
                source.clone(),
                tx.clone(),
                app_tx,
                timer_tx,
            ));

            // Timer event processor
            let sl_timer = session_layer.clone();
            tokio_with_wasm::spawn(async move {
                while let Some(event) = timer_rx.recv().await {
                    sl_timer.handle_timer_event(event).await;
                }
                debug!("timer event processor exited");
            });

            // App event drainer — delivers messages to JS callback
            let cb_app = on_message_cb.clone();
            tokio_with_wasm::spawn(async move {
                while let Some(event) = app_rx.recv().await {
                    let cb = cb_app.lock();
                    if let Some(callback) = cb.as_ref() {
                        match event {
                            AppEvent::Message(msg) => {
                                let js_obj = build_message_js_object(&msg);
                                if let Err(e) = callback.call1(&js_obj) {
                                    warn!("JS callback error: {:?}", e);
                                }
                            }
                            AppEvent::Error {
                                session_id,
                                error,
                                ..
                            } => {
                                let js_obj = build_error_js_object(session_id, &error);
                                if let Err(e) = callback.call1(&js_obj) {
                                    warn!("JS error callback error: {:?}", e);
                                }
                            }
                            AppEvent::MessageLost { session_id } => {
                                let js_obj =
                                    build_error_js_object(session_id, "message lost");
                                if let Err(e) = callback.call1(&js_obj) {
                                    warn!("JS lost callback error: {:?}", e);
                                }
                            }
                        }
                    } else {
                        debug!("received app event but no listener registered");
                    }
                }
                debug!("app event drainer exited");
            });

            // WebSocket read loop — dispatches everything through session layer
            let conn_flag_read = connected.clone();
            let sl_read = session_layer.clone();
            tokio_with_wasm::spawn(async move {
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(gloo_net::websocket::Message::Bytes(bytes)) => {
                            match Message::decode(bytes.as_slice()) {
                                Ok(msg) => {
                                    sl_read.dispatch_inbound(msg).await;
                                }
                                Err(e) => {
                                    warn!("failed to decode message: {e}");
                                }
                            }
                        }
                        Ok(gloo_net::websocket::Message::Text(_)) => {
                            debug!("ignoring text frame");
                        }
                        Err(e) => {
                            error!("ws read error: {e}");
                            break;
                        }
                    }
                }
                *conn_flag_read.lock() = false;
                debug!("read loop exited, connection marked closed");
            });

            // Auto-subscribe to own agent type
            let dest_type = AgentType::from_strings(org, ns, app);
            let self_subscribe =
                ProtoMessage::new_subscribe(&source, &dest_type, None, None);

            if let Err(e) = self_subscribe.validate() {
                return Err(JsValue::from_str(&format!(
                    "internal error: invalid subscribe message: {e}"
                )));
            }

            tx.send(self_subscribe)
                .await
                .map_err(|e| JsValue::from_str(&format!("self-subscribe failed: {e}")))?;

            let app_name = format!("{org}/{ns}/{app}");
            info!("connected and auto-subscribed as {app_name}");

            Ok(SlimClient {
                session_layer,
                org: org.to_string(),
                ns: ns.to_string(),
                app: app.to_string(),
                tx: Arc::new(Mutex::new(Some(tx))),
                on_message_cb,
                connected,
            })
        }

        #[wasm_bindgen(getter, js_name = "isConnected")]
        pub fn is_connected(&self) -> bool {
            *self.connected.lock()
        }

        #[wasm_bindgen(js_name = "subscribe")]
        pub async fn subscribe(
            &self,
            org: &str,
            ns: &str,
            name: &str,
        ) -> Result<(), JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }
            let source = self.session_layer.source();
            let dest_type = AgentType::from_strings(org, ns, name);
            let msg = ProtoMessage::new_subscribe(source, &dest_type, None, None);
            if let Err(e) = msg.validate() {
                return Err(JsValue::from_str(&format!("invalid message: {e}")));
            }
            self.send_msg(msg).await?;
            info!("subscribed to {org}/{ns}/{name}");
            Ok(())
        }

        #[wasm_bindgen(js_name = "unsubscribe")]
        pub async fn unsubscribe(
            &self,
            org: &str,
            ns: &str,
            name: &str,
        ) -> Result<(), JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }
            let source = self.session_layer.source();
            let dest_type = AgentType::from_strings(org, ns, name);
            let msg = ProtoMessage::new_unsubscribe(source, &dest_type, None, None);
            if let Err(e) = msg.validate() {
                return Err(JsValue::from_str(&format!("invalid message: {e}")));
            }
            self.send_msg(msg).await?;
            info!("unsubscribed from {org}/{ns}/{name}");
            Ok(())
        }

        /// Subscribe with recv_from flag (set_route equivalent).
        #[wasm_bindgen(js_name = "setRoute")]
        pub async fn set_route(
            &self,
            org: &str,
            ns: &str,
            name: &str,
            recv_from: f64,
        ) -> Result<(), JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }
            let source = self.session_layer.source();
            let dest_type = AgentType::from_strings(org, ns, name);
            let flags = SlimHeaderFlags::default().with_recv_from(recv_from as u64);
            let msg = ProtoMessage::new_subscribe(source, &dest_type, None, Some(flags));
            if let Err(e) = msg.validate() {
                return Err(JsValue::from_str(&format!("invalid message: {e}")));
            }
            self.send_msg(msg).await?;
            info!("set route to {org}/{ns}/{name} recv_from={recv_from}");
            Ok(())
        }

        /// Unsubscribe with recv_from flag (remove_route equivalent).
        #[wasm_bindgen(js_name = "removeRoute")]
        pub async fn remove_route(
            &self,
            org: &str,
            ns: &str,
            name: &str,
            recv_from: f64,
        ) -> Result<(), JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }
            let source = self.session_layer.source();
            let dest_type = AgentType::from_strings(org, ns, name);
            let flags = SlimHeaderFlags::default().with_recv_from(recv_from as u64);
            let msg = ProtoMessage::new_unsubscribe(source, &dest_type, None, Some(flags));
            if let Err(e) = msg.validate() {
                return Err(JsValue::from_str(&format!("invalid message: {e}")));
            }
            self.send_msg(msg).await?;
            info!("removed route to {org}/{ns}/{name} recv_from={recv_from}");
            Ok(())
        }

        /// Publish on a session (default content type).
        #[wasm_bindgen(js_name = "publish")]
        pub async fn publish(
            &self,
            session_id: u32,
            payload: &[u8],
        ) -> Result<(), JsValue> {
            self.publish_with_content_type(session_id, payload, "application/octet-stream")
                .await
        }

        /// Publish with explicit content type.
        #[wasm_bindgen(js_name = "publishWithContentType")]
        pub async fn publish_with_content_type(
            &self,
            session_id: u32,
            payload: &[u8],
            content_type: &str,
        ) -> Result<(), JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }
            self.session_layer
                .send_outbound(session_id, payload, content_type, None)
                .await
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Publish with custom flags (fanout, forward_to).
        #[wasm_bindgen(js_name = "publishWithFlags")]
        pub async fn publish_with_flags(
            &self,
            session_id: u32,
            payload: &[u8],
            content_type: &str,
            fanout: Option<u32>,
            forward_to: Option<f64>,
        ) -> Result<(), JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }
            let mut flags = SlimHeaderFlags::default();
            if let Some(f) = fanout {
                flags = flags.with_fanout(f);
            }
            if let Some(ft) = forward_to {
                flags = flags.with_forward_to(ft as u64);
            }
            self.session_layer
                .send_outbound(session_id, payload, content_type, Some(flags))
                .await
                .map_err(|e| JsValue::from_str(&e))
        }

        /// Send a reply for a RequestResponse session.
        #[wasm_bindgen(js_name = "sendReply")]
        pub async fn send_reply(
            &self,
            session_id: u32,
            message_id: u32,
            payload: &[u8],
            content_type: &str,
        ) -> Result<(), JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }
            self.session_layer
                .send_reply(session_id, message_id, payload, content_type)
                .await
                .map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = "listen")]
        pub fn listen(&self, on_message: js_sys::Function) -> Result<(), JsValue> {
            let mut cb = self.on_message_cb.lock();
            *cb = Some(JsCallback(on_message));
            info!("message listener registered");
            Ok(())
        }

        /// Create a session with full configuration.
        ///
        /// `session_type` accepts: "fnf", "reliable", "request-response", "streaming"
        /// `timeout_ms`: timeout in ms (default: 1000 for FNF/Streaming, 5000 for RR)
        /// `max_retries`: max retries (default: 5 for FNF, 10 for Streaming)
        /// `sticky`: enable sticky sessions (FNF reliable only)
        /// `direction`: streaming direction ("sender", "receiver", "bidirectional")
        #[wasm_bindgen(js_name = "createSession")]
        pub fn create_session(
            &self,
            dest_org: &str,
            dest_ns: &str,
            dest_app: &str,
            session_type: Option<String>,
            timeout_ms: Option<u32>,
            max_retries: Option<u32>,
            sticky: Option<bool>,
            direction: Option<String>,
        ) -> Result<u32, JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }

            let config = SessionConfig::from_js_params(
                session_type.as_deref(),
                timeout_ms,
                max_retries,
                sticky,
                direction.as_deref(),
            )
            .map_err(|e| JsValue::from_str(&e))?;

            let destination = AgentType::from_strings(dest_org, dest_ns, dest_app);
            let config_str = config.as_str().to_string();
            let id = self.session_layer.create_session(destination, config);

            info!(
                "created session {id} ({config_str}) -> {dest_org}/{dest_ns}/{dest_app}"
            );
            Ok(id)
        }

        #[wasm_bindgen(js_name = "deleteSession")]
        pub fn delete_session(&self, session_id: u32) -> Result<(), JsValue> {
            if self.session_layer.delete_session(session_id) {
                info!("deleted session {session_id}");
                Ok(())
            } else {
                Err(JsValue::from_str(&format!(
                    "session {session_id} not found"
                )))
            }
        }

        /// Get the session type string for an existing session.
        #[wasm_bindgen(js_name = "getSessionType")]
        pub fn get_session_type(&self, session_id: u32) -> Result<String, JsValue> {
            self.session_layer
                .get_session_info(session_id)
                .map(|(_, st)| st)
                .ok_or_else(|| JsValue::from_str(&format!("session {session_id} not found")))
        }

        /// Start sticky session discovery (FNF reliable sessions only).
        #[wasm_bindgen(js_name = "startStickyDiscovery")]
        pub async fn start_sticky_discovery(&self, session_id: u32) -> Result<(), JsValue> {
            if !self.is_connected() {
                return Err(JsValue::from_str("not connected"));
            }
            self.session_layer
                .start_sticky_discovery(session_id)
                .await
                .map_err(|e| JsValue::from_str(&e))
        }

        #[wasm_bindgen(js_name = "disconnect")]
        pub fn disconnect(&self) {
            {
                let mut tx = self.tx.lock();
                *tx = None;
            }
            *self.connected.lock() = false;
            info!("disconnected");
        }

        #[wasm_bindgen(getter, js_name = "appName")]
        pub fn app_name(&self) -> String {
            format!("{}/{}/{}", self.org, self.ns, self.app)
        }
    }

    // ---------------------------------------------------------------------------
    // GroupChat — MLS-based group encryption for WASM
    // ---------------------------------------------------------------------------

    #[wasm_bindgen]
    pub struct GroupChat {
        mls: Mls<SharedSecret, SharedSecret>,
    }

    #[wasm_bindgen]
    impl GroupChat {
        #[wasm_bindgen(constructor)]
        pub async fn new(app_id: &str, shared_secret: &str) -> Result<GroupChat, JsValue> {
            let provider = SharedSecret::new(app_id, shared_secret)
                .map_err(|e| JsValue::from_str(&format!("auth init failed: {e}")))?;
            let verifier = SharedSecret::new(app_id, shared_secret)
                .map_err(|e| JsValue::from_str(&format!("auth init failed: {e}")))?;

            let mut mls = Mls::new(provider, verifier);
            mls.initialize()
                .await
                .map_err(|e| JsValue::from_str(&format!("MLS init failed: {e}")))?;

            info!("GroupChat MLS initialized for {app_id}");
            Ok(GroupChat { mls })
        }

        #[wasm_bindgen(js_name = "createGroup")]
        pub async fn create_group(&mut self) -> Result<js_sys::Uint8Array, JsValue> {
            let group_id = self
                .mls
                .create_group()
                .await
                .map_err(|e| JsValue::from_str(&format!("create group failed: {e}")))?;
            info!("created MLS group");
            Ok(js_sys::Uint8Array::from(group_id.as_slice()))
        }

        #[wasm_bindgen(js_name = "generateKeyPackage")]
        pub async fn generate_key_package(&self) -> Result<js_sys::Uint8Array, JsValue> {
            let kp = self
                .mls
                .generate_key_package()
                .await
                .map_err(|e| JsValue::from_str(&format!("key package generation failed: {e}")))?;
            Ok(js_sys::Uint8Array::from(kp.as_slice()))
        }

        #[wasm_bindgen(js_name = "addMember")]
        pub async fn add_member(&mut self, key_package: &[u8]) -> Result<JsValue, JsValue> {
            let result = self
                .mls
                .add_member(key_package)
                .await
                .map_err(|e| JsValue::from_str(&format!("add member failed: {e}")))?;

            let obj = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("welcomeMessage"),
                &js_sys::Uint8Array::from(result.welcome_message.as_slice()),
            );
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("commitMessage"),
                &js_sys::Uint8Array::from(result.commit_message.as_slice()),
            );
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("memberIdentity"),
                &js_sys::Uint8Array::from(result.member_identity.as_slice()),
            );
            info!("member added to MLS group");
            Ok(obj.into())
        }

        #[wasm_bindgen(js_name = "removeMember")]
        pub async fn remove_member(&mut self, identity: &[u8]) -> Result<js_sys::Uint8Array, JsValue> {
            let commit = self
                .mls
                .remove_member(identity)
                .await
                .map_err(|e| JsValue::from_str(&format!("remove member failed: {e}")))?;
            info!("member removed from MLS group");
            Ok(js_sys::Uint8Array::from(commit.as_slice()))
        }

        #[wasm_bindgen(js_name = "joinGroup")]
        pub async fn join_group(&mut self, welcome_message: &[u8]) -> Result<js_sys::Uint8Array, JsValue> {
            let group_id = self
                .mls
                .process_welcome(welcome_message)
                .await
                .map_err(|e| JsValue::from_str(&format!("join group failed: {e}")))?;
            info!("joined MLS group");
            Ok(js_sys::Uint8Array::from(group_id.as_slice()))
        }

        #[wasm_bindgen(js_name = "processCommit")]
        pub async fn process_commit(&mut self, commit_message: &[u8]) -> Result<(), JsValue> {
            self.mls
                .process_commit(commit_message)
                .await
                .map_err(|e| JsValue::from_str(&format!("process commit failed: {e}")))
        }

        #[wasm_bindgen(js_name = "processProposal")]
        pub async fn process_proposal(
            &mut self,
            proposal_message: &[u8],
            create_commit: bool,
        ) -> Result<js_sys::Uint8Array, JsValue> {
            let commit = self
                .mls
                .process_proposal(proposal_message, create_commit)
                .await
                .map_err(|e| JsValue::from_str(&format!("process proposal failed: {e}")))?;
            Ok(js_sys::Uint8Array::from(commit.as_slice()))
        }

        #[wasm_bindgen(js_name = "encrypt")]
        pub async fn encrypt(&mut self, plaintext: &[u8]) -> Result<js_sys::Uint8Array, JsValue> {
            let encrypted = self
                .mls
                .encrypt_message(plaintext)
                .await
                .map_err(|e| JsValue::from_str(&format!("encrypt failed: {e}")))?;
            Ok(js_sys::Uint8Array::from(encrypted.as_slice()))
        }

        #[wasm_bindgen(js_name = "decrypt")]
        pub async fn decrypt(&mut self, ciphertext: &[u8]) -> Result<js_sys::Uint8Array, JsValue> {
            let decrypted = self
                .mls
                .decrypt_message(ciphertext)
                .await
                .map_err(|e| JsValue::from_str(&format!("decrypt failed: {e}")))?;
            Ok(js_sys::Uint8Array::from(decrypted.as_slice()))
        }

        #[wasm_bindgen(js_name = "rotateCredentials")]
        pub async fn rotate_credentials(&mut self) -> Result<js_sys::Uint8Array, JsValue> {
            let proposal = self
                .mls
                .create_rotation_proposal()
                .await
                .map_err(|e| JsValue::from_str(&format!("rotate credentials failed: {e}")))?;
            info!("credential rotation proposal created");
            Ok(js_sys::Uint8Array::from(proposal.as_slice()))
        }

        #[wasm_bindgen(getter, js_name = "groupId")]
        pub fn group_id(&self) -> Option<js_sys::Uint8Array> {
            self.mls
                .get_group_id()
                .map(|id| js_sys::Uint8Array::from(id.as_slice()))
        }

        #[wasm_bindgen(getter, js_name = "epoch")]
        pub fn epoch(&self) -> Option<f64> {
            self.mls.get_epoch().map(|e| e as f64)
        }
    }

    fn build_message_js_object(msg: &Message) -> JsValue {
        let obj = js_sys::Object::new();

        match &msg.message_type {
            Some(proto::message::MessageType::Publish(publish)) => {
                let _ = js_sys::Reflect::set(
                    &obj,
                    &JsValue::from_str("type"),
                    &JsValue::from_str("publish"),
                );

                if let Some(ref session) = publish.session {
                    let _ = js_sys::Reflect::set(
                        &obj,
                        &JsValue::from_str("sessionId"),
                        &JsValue::from(session.session_id),
                    );
                    let _ = js_sys::Reflect::set(
                        &obj,
                        &JsValue::from_str("messageId"),
                        &JsValue::from(session.message_id),
                    );

                    let header_type_str =
                        match SessionHeaderType::try_from(session.header_type) {
                            Ok(SessionHeaderType::Fnf) => "fnf",
                            Ok(SessionHeaderType::FnfReliable) => "fnf-reliable",
                            Ok(SessionHeaderType::FnfAck) => "fnf-ack",
                            Ok(SessionHeaderType::FnfDiscovery) => "fnf-discovery",
                            Ok(SessionHeaderType::FnfDiscoveryReply) => {
                                "fnf-discovery-reply"
                            }
                            Ok(SessionHeaderType::Request) => "request",
                            Ok(SessionHeaderType::Reply) => "reply",
                            Ok(SessionHeaderType::Stream) => "stream",
                            Ok(SessionHeaderType::PubSub) => "pubsub",
                            Ok(SessionHeaderType::RtxRequest) => "rtx-request",
                            Ok(SessionHeaderType::RtxReply) => "rtx-reply",
                            Ok(SessionHeaderType::BeaconStream) => "beacon-stream",
                            Ok(SessionHeaderType::BeaconPubSub) => "beacon-pubsub",
                            _ => "unknown",
                        };
                    let _ = js_sys::Reflect::set(
                        &obj,
                        &JsValue::from_str("headerType"),
                        &JsValue::from_str(header_type_str),
                    );
                }

                if let Some(ref content) = publish.msg {
                    let payload = js_sys::Uint8Array::from(content.blob.as_slice());
                    let _ =
                        js_sys::Reflect::set(&obj, &JsValue::from_str("payload"), &payload);
                    let _ = js_sys::Reflect::set(
                        &obj,
                        &JsValue::from_str("contentType"),
                        &JsValue::from_str(&content.content_type),
                    );
                }

                if let Some(ref header) = publish.header {
                    set_source_fields(&obj, header);
                }
            }
            Some(proto::message::MessageType::Subscribe(sub)) => {
                let _ = js_sys::Reflect::set(
                    &obj,
                    &JsValue::from_str("type"),
                    &JsValue::from_str("subscribe"),
                );
                if let Some(ref header) = sub.header {
                    set_source_fields(&obj, header);
                }
            }
            Some(proto::message::MessageType::Unsubscribe(unsub)) => {
                let _ = js_sys::Reflect::set(
                    &obj,
                    &JsValue::from_str("type"),
                    &JsValue::from_str("unsubscribe"),
                );
                if let Some(ref header) = unsub.header {
                    set_source_fields(&obj, header);
                }
            }
            _ => {
                let _ = js_sys::Reflect::set(
                    &obj,
                    &JsValue::from_str("type"),
                    &JsValue::from_str("unknown"),
                );
            }
        }

        obj.into()
    }

    fn build_error_js_object(session_id: u32, error: &str) -> JsValue {
        let obj = js_sys::Object::new();
        let _ = js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("type"),
            &JsValue::from_str("error"),
        );
        let _ = js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("sessionId"),
            &JsValue::from(session_id),
        );
        let _ = js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("error"),
            &JsValue::from_str(error),
        );
        obj.into()
    }

    fn set_source_fields(obj: &js_sys::Object, header: &proto::SlimHeader) {
        if let Some(ref src) = header.source {
            let source = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &source,
                &JsValue::from_str("organization"),
                &JsValue::from(src.organization as f64),
            );
            let _ = js_sys::Reflect::set(
                &source,
                &JsValue::from_str("namespace"),
                &JsValue::from(src.namespace as f64),
            );
            let _ = js_sys::Reflect::set(
                &source,
                &JsValue::from_str("agentType"),
                &JsValue::from(src.agent_type as f64),
            );
            let _ = js_sys::Reflect::set(obj, &JsValue::from_str("source"), &source);
        }
    }
}
