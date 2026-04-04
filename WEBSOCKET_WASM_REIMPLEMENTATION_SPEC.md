# WebSocket + WASM Data-Plane — Complete Reimplementation Spec

> Generated from branch `feat/websocket-server-client` diff against `main` (merge-base `2fb31419`).
> **Reference patch**: `websocket-wasm-vs-main.patch` (repo root, ~673 KB, 132 files, ~9.3k insertions / ~3.9k deletions).
> Use this spec as the primary contract and the patch as the line-level reference.

---

## Table of Contents

1. [High-Level Goals](#1-high-level-goals)
2. [Architecture & Feature Gating Convention](#2-architecture--feature-gating-convention)
3. [Workspace-Level Changes](#3-workspace-level-changes)
4. [Crate-by-Crate Implementation Guide](#4-crate-by-crate-implementation-guide)
   - 4.1 [agntcy-slim-config](#41-agntcy-slim-config-coreconfig)
   - 4.2 [agntcy-slim-auth](#42-agntcy-slim-auth-coreauth)
   - 4.3 [agntcy-slim-datapath](#43-agntcy-slim-datapath-coredatapath)
   - 4.4 [agntcy-slim-mls](#44-agntcy-slim-mls-coremls)
   - 4.5 [agntcy-slim-tracing](#45-agntcy-slim-tracing-coretracing)
   - 4.6 [agntcy-slim-session](#46-agntcy-slim-session-coresession)
   - 4.7 [agntcy-slim-controller](#47-agntcy-slim-controller-corecontroller)
   - 4.8 [agntcy-slim-signal](#48-agntcy-slim-signal-coresignal)
   - 4.9 [agntcy-slim-service](#49-agntcy-slim-service-coreservice)
   - 4.10 [agntcy-slim-wasm](#410-agntcy-slim-wasm-coreslim-wasm)
5. [Config YAML Samples](#5-config-yaml-samples)
6. [CI / Automation](#6-ci--automation)
7. [Examples & Browser Demo](#7-examples--browser-demo)
8. [Verification Commands](#8-verification-commands)
9. [Known WASM Limitations](#9-known-wasm-limitations)

---

## 1. High-Level Goals

1. **Native WebSocket transport** — data-plane clients and servers can use WebSocket (alongside existing gRPC), with YAML config, TLS support, and browser-compatible auth (token in query string because browsers cannot set custom WS handshake headers).
2. **WASM target `wasm32-unknown-unknown`** — all core crates compile with `--no-default-features --features wasm`. No Tokio on WASM paths.
3. **`agntcy-slim-wasm` crate** — `wasm-bindgen` API (`SlimClient`) for browser: connect via `gloo-net` WebSocket, HMAC auth, subscribe/unsubscribe, sessions (P2P + multicast), MLS optional, publish, participants, notifications, disconnect. Console tracing.
4. **CI job** — installs `wasm32-unknown-unknown`, runs Taskfile target that `cargo check`s all WASM-ready crates.
5. **Examples** — Taskfile tasks for ws/wss examples; browser demo HTML for manual E2E testing.
6. **Session runtime abstraction** — `runtime.rs` module so session layer works on both Tokio (native) and browser (WASM) runtimes.
7. **Config crate refactor** — shared `client`, `server`, `tls`, `websocket` modules extracted from gRPC-only code; schemas moved to `src/schema/`.

### Non-goals / constraints

- Browser **cannot** host a WebSocket **server**; WS server is native-only.
- Do **not** remove any native/gRPC functionality. Always use feature gates.
- Go control-plane version bumps are **orthogonal** — skip unless your `main` already standardized.

---

## 2. Architecture & Feature Gating Convention

Every data-plane crate uses two mutually exclusive features:

```toml
[features]
default = ["native"]
native = [ ... ]  # tokio, tonic, aws-lc-rs, etc.
wasm   = [ ... ]  # gloo-net, futures-channel, wasm-bindgen, etc.
```

**Conditional compilation pattern used everywhere:**

```rust
#[cfg(feature = "native")]
// native-only code

#[cfg(all(feature = "wasm", not(feature = "native")))]
// wasm-only code
```

The `not(feature = "native")` guard ensures that when both features are active (e.g., workspace builds), native wins.

**Trait bounds pattern for async_trait:**

```rust
#[cfg_attr(feature = "native", async_trait)]
#[cfg_attr(feature = "wasm", async_trait(?Send))]
pub trait MyTrait { ... }
```

**MaybeSend / MaybeSync marker traits** (in `session/src/traits.rs`):

```rust
#[cfg(feature = "native")]
pub trait MaybeSend: Send {}
#[cfg(feature = "native")]
impl<T: Send> MaybeSend for T {}

#[cfg(all(feature = "wasm", not(feature = "native")))]
pub trait MaybeSend {}
#[cfg(all(feature = "wasm", not(feature = "native")))]
impl<T> MaybeSend for T {}
```

Same pattern for `MaybeSync`.

---

## 3. Workspace-Level Changes

### 3.1 `data-plane/Cargo.toml` — workspace members

Add `core/slim-wasm` to `[workspace.members]`. Do NOT add it to `default-members` (it only compiles for wasm32).

### 3.2 `data-plane/.cargo/config.toml`

Add this section (required for MLS WebCrypto):

```toml
[target.wasm32-unknown-unknown]
rustflags = ["--cfg", "mls_build_async"]
```

### 3.3 Workspace dependencies to add

In `[workspace.dependencies]`:

```toml
# Already in workspace (verify versions match main):
futures-channel = ...  # if not present
futures-core = ...     # if not present
gloo-net = "0.6"
gloo-timers = { version = "0.3", features = ["futures"] }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
js-sys = "0.3"
web-sys = { version = "0.3", features = ["console"] }
console_error_panic_hook = "0.1"
mls-rs-crypto-webcrypto = "0.14"
getrandom = { version = "0.3", features = ["wasm_js"] }
hmac = "0.12"
sha2 = "0.10"
maybe-async = "0.2.10"
```

---

## 4. Crate-by-Crate Implementation Guide

### 4.1 `agntcy-slim-config` (`core/config`)

#### 4.1.1 Features

```toml
[features]
default = ["native"]
native = [
    "dep:agntcy-slim-auth", "dep:bytes", "dep:display-error-chain", "dep:drain",
    "dep:fastwebsockets", "dep:futures", "dep:http-body-util", "dep:hyper",
    "hyper-util/client-proxy-system", "dep:hyper-rustls", "dep:hyper-util",
    "dep:parking_lot", "dep:prost", "dep:rustls", "dep:rustls-native-certs",
    "dep:rustls-pki-types", "dep:tokio", "tokio/io-util", "tokio/macros",
    "tokio/net", "tokio/rt", "tokio/rt-multi-thread", "tokio/sync", "tokio/time",
    "dep:tokio-retry", "dep:tokio-rustls", "dep:tokio-stream", "dep:tokio-util",
    "dep:tonic", "dep:tonic-prost", "dep:tonic-tls",
    "dep:tower-layer", "dep:tower-service",
]
wasm = ["dep:gloo-net", "uuid/js"]
```

Key addition: `gloo-net = { version = "0.6", optional = true }` in `[dependencies]`.

#### 4.1.2 Module structure changes

**`src/lib.rs`** — add new top-level modules:

```rust
pub mod auth;
pub mod backoff;
pub mod client;       // NEW — shared client config (extracted from grpc/client.rs)
pub mod component;
pub mod grpc;
pub mod provider;
pub mod server;       // NEW — shared server config (extracted from grpc/server.rs)
#[cfg(feature = "native")]
pub mod testutils;
pub mod tls;          // NEW — TLS config types
pub mod transport;
pub mod websocket;    // NEW — WebSocket config types

mod opaque;

pub const CLIENT_CONFIG_SCHEMA_JSON: &str = include_str!("./schema/client-config.schema.json");
pub const SERVER_CONFIG_SCHEMA_JSON: &str = include_str!("./schema/server-config.schema.json");
```

**Move JSON schema files** from `src/grpc/schema/` to `src/schema/`.

#### 4.1.3 New modules

**`src/client.rs`** — shared `ClientConfig` struct with fields:
- `endpoint: String`
- `transport: Transport` (enum: `Grpc`, `WebSocket`)
- `tls_setting: TlsClientConfig`
- `websocket_auth_query_param: Option<String>` (for browser WS auth — token sent as `?token=xxx` in URL)
- Connection params (backoff, keepalive, etc.)
- `validate()` method

**`src/server.rs`** — shared `ServerConfig` struct with:
- `endpoint: String`
- `transport: Transport`
- `tls_setting: TlsServerConfig`
- `websocket_auth_query_param: Option<String>`
- Keepalive, max connections, etc.
- `validate()` method

**`src/tls.rs`** — module entry:
```rust
pub mod client;  // TlsClientConfig
pub mod server;  // TlsServerConfig
```

**`src/tls/client.rs`** — `TlsClientConfig` with `insecure`, `ca_file`, `cert_file`, `key_file` fields.

**`src/tls/server.rs`** — `TlsServerConfig` with `insecure`, `cert_file`, `key_file`, `client_ca_file` fields.

**`src/websocket.rs`** — module router with feature-based path selection:
```rust
#[cfg(feature = "native")]
#[path = "websocket/client.rs"]
pub mod client;

#[cfg(all(feature = "wasm", not(feature = "native")))]
#[path = "websocket/client_wasm.rs"]
pub mod client;

#[cfg(feature = "native")]
pub mod common;

#[cfg(all(feature = "wasm", not(feature = "native")))]
#[path = "websocket/common_wasm.rs"]
pub mod common;

#[cfg(feature = "native")]
pub mod server;
```

**`src/websocket/common.rs`** (native) — contains:
- `Transport` enum (`Grpc`, `WebSocket`)
- `UpgradedWebSocket` type alias (for `fastwebsockets::WebSocket`)
- WebSocket upgrade logic (HTTP/1.1 upgrade handshake via hyper)
- Server-side WS accept with optional TLS
- Client-side WS connect with optional TLS
- Auth token extraction from query params
- Connection management helpers

**`src/websocket/common_wasm.rs`** (WASM) — minimal subset:
- `Transport` enum (same)
- No `UpgradedWebSocket` (browser handles WS natively)
- Stubs or reduced API surface

**`src/websocket/client.rs`** (native) — WebSocket client connection logic using `fastwebsockets` + `hyper`

**`src/websocket/client_wasm.rs`** (WASM) — WebSocket client using `gloo-net`

**`src/websocket/server.rs`** (native only) — WebSocket server accept logic using `fastwebsockets` + `hyper`

#### 4.1.4 `src/grpc/` changes

- `src/grpc/client.rs` — thin down; delegate shared config to `src/client.rs`
- `src/grpc/server.rs` — thin down; delegate shared config to `src/server.rs`
- `src/grpc/errors.rs` — add `ConfigError` variants for WebSocket
- `src/grpc/proxy.rs` — minor adjustments if needed

#### 4.1.5 Backoff changes

`src/backoff/exponential.rs` and `src/backoff/fixedinterval.rs` — gate tokio-specific retry logic behind `#[cfg(feature = "native")]`.

---

### 4.2 `agntcy-slim-auth` (`core/auth`)

#### 4.2.1 Features

```toml
[features]
default = ["native"]
native = [
    "dep:aws-lc-rs", "dep:display-error-chain", "dep:futures", "dep:headers",
    "dep:jsonwebtoken", "dep:mls-rs-core", "dep:mls-rs-crypto-awslc",
    "dep:notify", "dep:oauth2", "dep:parking_lot", "dep:pin-project",
    "dep:reqwest", "dep:tokio", "dep:tokio-util", "dep:tower", "dep:tower-layer",
    "dep:tower-service", "dep:url", "dep:wiremock",
    "native-spiffe",
]
native-spiffe = ["dep:spiffe"]
wasm = [
    "dep:hmac", "dep:js-sys", "dep:parking_lot", "dep:sha2", "dep:getrandom",
]
```

#### 4.2.2 `src/lib.rs` — feature gate modules

```rust
#[cfg(feature = "native")]
pub mod auth_provider;
#[cfg(feature = "native")]
pub mod builder;
pub mod errors;                       // always available
#[cfg(feature = "native")]
pub mod file_watcher;
pub mod identity_claims;              // always available
#[cfg(feature = "native")]
pub mod jwt;
#[cfg(feature = "native")]
pub mod jwt_middleware;
pub mod metadata;                     // always available
#[cfg(feature = "native")]
pub mod oidc;
#[cfg(feature = "native")]
pub mod resolver;
pub mod shared_secret;                // always available (WASM uses hmac+sha2)
#[cfg(all(feature = "native", not(target_family = "windows")))]
pub mod spire;
pub mod traits;                       // always available
#[cfg(feature = "native")]
pub mod utils;
```

#### 4.2.3 `src/shared_secret.rs` — dual HMAC implementation

The `SharedSecret` struct must work on both native and WASM:

- **Native**: Uses `aws-lc-rs` HMAC-SHA256
- **WASM**: Uses `hmac` + `sha2` crates (pure Rust)

Pattern:

```rust
#[cfg(feature = "native")]
fn hmac_sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    // aws-lc-rs HMAC
}

#[cfg(all(feature = "wasm", not(feature = "native")))]
fn hmac_sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}
```

Token generation uses `SystemTime` on native, `js_sys::Date::now()` on WASM for timestamp.

#### 4.2.4 `src/errors.rs` — gate native-only error variants

```rust
#[cfg(feature = "native")]
#[error("unsupported key algorithm: {0}")]
JwtUnsupportedKeyAlgorithm(KeyAlgorithm),

// ... etc — all JWT, OIDC, SPIRE, reqwest, notify, jsonwebtoken errors gated
```

#### 4.2.5 `src/traits.rs` — async_trait gating

```rust
#[cfg_attr(feature = "native", async_trait)]
#[cfg_attr(feature = "wasm", async_trait(?Send))]
pub trait Verifier { ... }

#[cfg_attr(feature = "native", async_trait)]
#[cfg_attr(feature = "wasm", async_trait(?Send))]
pub trait TokenProvider { ... }
```

#### 4.2.6 Other files

- `src/resolver.rs` — gate behind `#[cfg(feature = "native")]` (uses reqwest, URL)
- `src/spire.rs` — gate behind `#[cfg(all(feature = "native", not(target_family = "windows")))]`
- `src/jwt.rs` — gate behind `#[cfg(feature = "native")]` (uses jsonwebtoken)
- `src/auth_provider.rs` — gate behind `#[cfg(feature = "native")]`

---

### 4.3 `agntcy-slim-datapath` (`core/datapath`)

#### 4.3.1 Features

```toml
[features]
default = ["native"]
native = [
    "dep:agntcy-slim-config", "dep:agntcy-slim-tracing", "dep:display-error-chain",
    "dep:drain", "dep:fastwebsockets", "dep:h2", "dep:opentelemetry",
    "dep:tokio", "dep:tokio-stream", "dep:tokio-util",
    "dep:tonic", "dep:tonic-prost", "dep:tracing-opentelemetry",
]
wasm = ["dep:getrandom", "uuid/js"]
```

#### 4.3.2 `src/lib.rs` — gate heavy modules, add Status shim

```rust
pub mod api;
pub mod errors;
pub mod messages;
pub mod tables;

#[cfg(feature = "native")]
pub mod message_processing;
#[cfg(feature = "native")]
mod connection;
#[cfg(feature = "native")]
mod forwarder;
#[cfg(feature = "native")]
pub(crate) mod subscription_ack;
#[cfg(feature = "native")]
mod websocket;                        // NEW — native WebSocket transport

#[cfg(feature = "native")]
pub use tonic::Status;

/// Lightweight Status shim for WASM (no tonic available)
#[cfg(not(feature = "native"))]
#[derive(Debug, Clone)]
pub struct Status {
    code: u32,
    message: String,
}

#[cfg(not(feature = "native"))]
impl Status {
    pub fn new(code: u32, message: impl Into<String>) -> Self { ... }
    pub fn internal(message: impl Into<String>) -> Self { Self::new(13, message) }
    pub fn code(&self) -> u32 { self.code }
    pub fn message(&self) -> &str { &self.message }
}
// + Display, Error impls
```

#### 4.3.3 `src/api.rs` — gate gRPC service re-exports

```rust
// Proto message types — always available (pure prost)
pub use proto::dataplane::v1::*;  // all message types

// gRPC service types — native only
#[cfg(feature = "native")]
pub use proto::dataplane::v1::data_plane_service_client::DataPlaneServiceClient;
#[cfg(feature = "native")]
pub use proto::dataplane::v1::data_plane_service_server::DataPlaneServiceServer;
```

#### 4.3.4 `src/api/proto.rs` — add cfg attribute on generated module

```rust
// The generated file includes cfg-gated modules:
#[cfg(feature = "native")]  // on gRPC client/server modules
```

#### 4.3.5 `build.rs` — gate generated gRPC stubs

```rust
tonic_prost_build::configure()
    .out_dir("src/api/gen")
    .server_mod_attribute("dataplane.proto.v1", "#[cfg(feature = \"native\")]")
    .client_mod_attribute("dataplane.proto.v1", "#[cfg(feature = \"native\")]")
    .compile_protos(&["proto/v1/data_plane.proto"], &["proto/v1"])
    .unwrap();
```

#### 4.3.6 New: `src/websocket/` module (native only)

**`src/websocket.rs`**:
```rust
pub(crate) mod stream;
pub(crate) use stream::spawn_transport_tasks;
```

**`src/websocket/stream.rs`** — Spawns read/write tokio tasks for a `fastwebsockets::WebSocket`:
- Read loop: decodes binary frames as protobuf `Message`, sends to `mpsc::Sender`
- Write loop: receives `Message` from `mpsc::Receiver`, encodes and writes binary frames
- Both loops respect a `CancellationToken`
- Returns `WebSocketStreams { inbound: ReceiverStream, outbound: Sender }`

#### 4.3.7 `src/errors.rs` — gate tonic/native-specific variants

```rust
#[cfg(feature = "native")]
#[error("gRPC error: {0}")]
GrpcError(#[from] tonic::Status),
```

#### 4.3.8 `src/message_processing.rs` — add WebSocket transport support

The `connect()` and `run_server()` methods need to handle `Transport::WebSocket` in addition to `Transport::Grpc`:
- For client: use `websocket::spawn_transport_tasks` instead of gRPC streaming
- For server: accept WebSocket upgrades via hyper, then spawn transport tasks

---

### 4.4 `agntcy-slim-mls` (`core/mls`)

#### 4.4.1 Features

```toml
[features]
default = ["native"]
native = [
    "agntcy-slim-auth/native", "agntcy-slim-datapath/native",
    "dep:mls-rs-crypto-awslc", "dep:tokio", "tokio/macros", "tokio/rt",
]
wasm = [
    "agntcy-slim-auth/wasm", "agntcy-slim-datapath/wasm",
    "dep:mls-rs-crypto-webcrypto", "dep:getrandom",
]
```

Add to `[lints.rust.unexpected_cfgs]`:
```toml
level = "warn"
check-cfg = ["cfg(mls_build_async)"]
```

#### 4.4.2 New: `src/crypto.rs` — pluggable crypto provider

```rust
#[cfg(feature = "native")]
pub use mls_rs_crypto_awslc::AwsLcCryptoProvider as CryptoProviderImpl;

#[cfg(all(feature = "wasm", not(feature = "native")))]
pub use mls_rs_crypto_webcrypto::WebCryptoProvider as CryptoProviderImpl;

pub fn default_crypto_provider() -> CryptoProviderImpl {
    CryptoProviderImpl::default()
}
```

#### 4.4.3 `src/lib.rs` — add crypto module

```rust
pub mod crypto;
pub mod errors;
pub mod identity_provider;
pub mod mls;
```

#### 4.4.4 `src/mls.rs` — use crypto abstraction

Replace direct `AwsLcCryptoProvider` usage with:
```rust
use crate::crypto::CryptoProviderImpl;
```

#### 4.4.5 `src/identity_provider.rs` — fix Send bounds for WASM

The `IdentityProvider` trait from `mls-rs` requires `Send` futures. On WASM, `Verifier::get_claims()` returns `!Send` futures. Fix:

```rust
// Gate the async fallback path that calls get_claims() behind native.
// On WASM, try_get_claims() (synchronous HMAC) always succeeds,
// so the async path is unreachable.
fn resolve_slim_identity(&self, signing_id: &SigningIdentity) -> Result<IdentityClaims, MlsError> {
    // ... uses try_get_claims (sync) which works on both platforms
}
```

The `#[maybe_async::maybe_async(AFIT)]` or conditional compilation ensures WASM never hits the async `get_claims` path.

---

### 4.5 `agntcy-slim-tracing` (`core/tracing`)

#### 4.5.1 Features

```toml
[features]
default = ["native"]
native = [
    "dep:agntcy-slim-config", "dep:opentelemetry", "dep:opentelemetry-otlp",
    "dep:opentelemetry-semantic-conventions", "dep:opentelemetry-stdout",
    "dep:opentelemetry_sdk", "dep:tracing-opentelemetry", "dep:tracing-subscriber",
]
wasm = [
    "dep:tracing-subscriber", "dep:getrandom", "dep:web-sys", "uuid/js",
]
```

#### 4.5.2 `src/lib.rs` — conditional module loading

```rust
pub mod utils;

#[cfg(feature = "native")]
mod native;
#[cfg(feature = "native")]
pub use native::*;

#[cfg(all(feature = "wasm", not(feature = "native")))]
mod wasm;
#[cfg(all(feature = "wasm", not(feature = "native")))]
pub use wasm::*;
```

#### 4.5.3 New: `src/native.rs`

Move ALL existing tracing setup code (OpenTelemetry, subscribers, etc.) into this file unchanged. Exports `TracingConfiguration`, `OtelGuard`, `ConfigError`.

#### 4.5.4 New: `src/wasm.rs`

Lightweight tracing for browser. Key points:
- `ConsoleWriter` — implements `std::io::Write`, buffers log line, flushes via `web_sys::console::log_1()`
- `ConsoleMakeWriter` — implements `fmt::MakeWriter`
- `TracingConfiguration` — same API surface as native (`with_log_level`, `setup_tracing_subscriber`, etc.)
- `OtelGuard` — empty struct (nothing to shut down on WASM)
- No OpenTelemetry dependency

#### 4.5.5 New: `src/utils.rs`

```rust
use once_cell::sync::Lazy;
use uuid::Uuid;

#[cfg(not(target_arch = "wasm32"))]
pub static INSTANCE_ID: Lazy<String> =
    Lazy::new(|| std::env::var("SLIM_INSTANCE_ID").unwrap_or_else(|_| Uuid::new_v4().to_string()));

#[cfg(target_arch = "wasm32")]
pub static INSTANCE_ID: Lazy<String> =
    Lazy::new(|| Uuid::new_v4().to_string());
```

---

### 4.6 `agntcy-slim-session` (`core/session`)

This is the most extensively changed crate. The session layer must work on both Tokio and browser runtimes.

#### 4.6.1 Features

```toml
[features]
default = ["native"]
native = [
    "agntcy-slim-auth/native", "agntcy-slim-datapath/native", "agntcy-slim-mls/native",
    "dep:display-error-chain", "dep:futures", "dep:futures-timer",
    "dep:tokio", "dep:tokio-util", "dep:tonic",
]
wasm = [
    "agntcy-slim-auth/wasm", "agntcy-slim-datapath/wasm", "agntcy-slim-mls/wasm",
    "dep:getrandom", "dep:futures-channel", "dep:futures-core",
    "dep:gloo-timers", "dep:wasm-bindgen-futures",
]
```

Add `maybe-async = "0.2.10"` as always-available dependency.

Add to `[lints.rust.unexpected_cfgs]`:
```toml
level = "warn"
check-cfg = ["cfg(mls_build_async)"]
```

#### 4.6.2 New: `src/runtime.rs` — THE KEY ABSTRACTION

This is ~644 lines providing platform-agnostic async primitives. Must implement:

**Channels:**
- `runtime::channel::mpsc::channel(buffer)` — native: `tokio::sync::mpsc::channel`; WASM: `futures_channel::mpsc::unbounded` (ignores buffer)
- `runtime::channel::mpsc::unbounded_channel()` — native: `tokio::sync::mpsc::unbounded_channel`; WASM: `futures_channel::mpsc::unbounded`
- `runtime::channel::oneshot::channel()` — native: `tokio::sync::oneshot`; WASM: `futures_channel::oneshot`
- WASM channel wrappers must match tokio's API surface: `Sender::send()` (async), `Receiver::recv()` (async), `UnboundedSender::send()` (sync), `try_recv()`, `SendError<T>`, `TryRecvError`
- WASM `Receiver` implements `futures_core::Stream`

**Spawn:**
- `runtime::spawn(future)` — native: `tokio::spawn`; WASM: `wasm_bindgen_futures::spawn_local` returning a fire-and-forget `JoinHandle`
- WASM `JoinHandle<T>` — `abort()` is no-op; `Future::poll` always returns `Pending`; `JoinError` stub

**CancellationToken:**
- Native: re-export `tokio_util::sync::CancellationToken`
- WASM: custom implementation with `AtomicBool` + `Mutex<Vec<Waker>>`. Methods: `new()`, `cancel()`, `is_cancelled()`, `child_token()` (clone), `cancelled()` (async future), `register_waker()`

**Time:**
- `runtime::sleep(duration)` — native: `tokio::time::sleep`; WASM: `gloo_timers::future::TimeoutFuture::new(millis)`
- `runtime::Duration` — re-export `std::time::Duration`

**Select helpers:**
- `select_timer_or_cancel(duration, token)` — returns `SelectResult::TimerExpired` or `Cancelled`
- `select_recv_or_cancel(rx, token)` — returns `Option<T>`
- `select_processing(rx, token, deadline, check_cancel)` — 3-way select for session processing loop; returns `ProcessingSelect<T>` enum

**Deadline (resettable timer):**
- Native: wraps `Pin<Box<tokio::time::Sleep>>` with `reset()`, `poll_expired()`
- WASM: tracks remaining_ms + `Option<Pin<Box<gloo_timers::future::TimeoutFuture>>>`; large durations (>1 day) treated as infinite

**Status re-export:**
```rust
pub use slim_datapath::Status;
```

#### 4.6.3 `src/lib.rs` — add runtime module

```rust
pub mod runtime;  // ADD as first module
// ... rest unchanged
```

#### 4.6.4 `src/traits.rs` — add MaybeSend/MaybeSync, gate async_trait

As described in section 2. The `Transmitter` and `MessageHandler` traits use:
```rust
#[cfg_attr(feature = "native", async_trait)]
#[cfg_attr(feature = "wasm", async_trait(?Send))]
```

#### 4.6.5 Changes across session sub-modules

Every file in the session crate that uses tokio must be updated to use `crate::runtime::` instead:

| tokio API | runtime replacement |
|-----------|-------------------|
| `tokio::sync::mpsc::channel` | `runtime::channel::mpsc::channel` |
| `tokio::sync::mpsc::Sender` | `runtime::channel::mpsc::Sender` |
| `tokio::sync::oneshot::channel` | `runtime::channel::oneshot::channel` |
| `tokio::spawn` | `runtime::spawn` |
| `tokio::time::sleep` | `runtime::sleep` |
| `tokio_util::sync::CancellationToken` | `runtime::CancellationToken` |
| `tokio::select!` | `runtime::select_*` helpers or manual `poll_fn` |
| `tonic::Status` | `runtime::Status` (or `slim_datapath::Status`) |

**Files requiring these changes** (all in `core/session/src/`):
- `common.rs` — `SlimChannelSender` type alias uses runtime channel types
- `completion_handle.rs` — uses runtime oneshot
- `context.rs` — uses runtime channels
- `controller_sender.rs` — uses runtime spawn, channels, select helpers, Deadline
- `errors.rs` — gate tonic::Status behind native
- `interceptor.rs` — gate async_trait
- `mls_state.rs` — uses runtime spawn, channels
- `moderator_task.rs` — uses runtime spawn, select
- `session.rs` — uses runtime spawn
- `session_builder.rs` — uses runtime channels
- `session_controller.rs` — uses runtime channels, spawn
- `session_layer.rs` — uses runtime channels, spawn; gate `Direction` usage
- `session_moderator.rs` — uses runtime Deadline, select_processing
- `session_participant.rs` — uses runtime select_processing
- `session_receiver.rs` — uses runtime select_processing, Deadline
- `session_sender.rs` — uses runtime select_processing, Deadline
- `session_settings.rs` — minor
- `subscription_manager.rs` — uses runtime channels, spawn
- `timer.rs` — uses runtime Deadline, sleep
- `timer_factory.rs` — uses runtime sleep, spawn
- `transmitter.rs` — gate async_trait

---

### 4.7 `agntcy-slim-controller` (`core/controller`)

#### 4.7.1 Features

```toml
[features]
default = ["native"]
native = [
    "agntcy-slim-auth/native", "agntcy-slim-config/native",
    "agntcy-slim-datapath/native", "agntcy-slim-session/native",
    "agntcy-slim-signal/native", "agntcy-slim-tracing/native",
    "display-error-chain", "drain", "h2",
    "tokio", "tokio-stream", "tokio-util", "tonic", "tonic-prost",
]
wasm = [
    "agntcy-slim-auth/wasm", "agntcy-slim-config/wasm",
    "agntcy-slim-datapath/wasm", "agntcy-slim-session/wasm",
    "agntcy-slim-tracing/wasm",
]
```

Note: `agntcy-slim-signal` is **optional** and only needed for native.

#### 4.7.2 `build.rs` — gate generated gRPC stubs

Same pattern as datapath:
```rust
tonic_prost_build::configure()
    .out_dir("src/api/gen")
    .server_mod_attribute("controller.proto.v1", "#[cfg(feature = \"native\")]")
    .client_mod_attribute("controller.proto.v1", "#[cfg(feature = \"native\")]")
    .compile_protos(...)
```

**Important**: The module path must be `controller.proto.v1` (not `controller.v1`). The generated file is `src/api/gen/controller.proto.v1.rs`.

#### 4.7.3 `src/lib.rs`

```rust
pub mod api;
#[cfg(feature = "native")]
pub mod config;
pub mod errors;
#[cfg(feature = "native")]
pub mod service;
```

#### 4.7.4 `src/errors.rs` — gate native variants

```rust
#[cfg(feature = "native")]
#[error("configuration error")]
ConfigError(#[from] ConfigError),
#[cfg(feature = "native")]
#[error("grpc error")]
GrpcError(#[from] Status),
```

---

### 4.8 `agntcy-slim-signal` (`core/signal`)

#### 4.8.1 Features

```toml
[features]
default = ["native"]
native = ["tokio"]
wasm = []
```

#### 4.8.2 `src/lib.rs` — add WASM stub

```rust
pub async fn shutdown() { imp::shutdown().await }

#[cfg(all(feature = "native", unix))]
mod imp { /* tokio signal handlers */ }

#[cfg(all(feature = "native", not(unix)))]
mod imp { /* windows ctrl_c */ }

#[cfg(all(feature = "wasm", not(feature = "native")))]
mod imp {
    pub(super) async fn shutdown() {
        std::future::pending::<()>().await  // WASM: no OS signals, pend forever
    }
}
```

---

### 4.9 `agntcy-slim-service` (`core/service`)

#### 4.9.1 Features

```toml
[features]
default = ["native", "session"]
native = [
    "agntcy-slim-auth/native", "agntcy-slim-config/native",
    "agntcy-slim-controller/native", "agntcy-slim-datapath/native",
    "agntcy-slim-mls/native", "agntcy-slim-session?/native",
    "display-error-chain", "tokio", "tokio-util",
]
wasm = [
    "agntcy-slim-auth/wasm", "agntcy-slim-config/wasm",
    "agntcy-slim-controller/wasm", "agntcy-slim-datapath/wasm",
    "agntcy-slim-mls/wasm", "agntcy-slim-session?/wasm",
]
session = ["agntcy-slim-session"]
```

Note the `agntcy-slim-session?/native` syntax — the `?` means "only activate if `agntcy-slim-session` is already enabled by the `session` feature".

#### 4.9.2 `src/lib.rs` — gate Service behind native

```rust
pub mod errors;
#[cfg(feature = "native")]
#[macro_use]
pub mod service;

#[cfg(all(feature = "native", feature = "session"))]
pub mod app;

pub use slim_datapath::messages::utils::SlimHeaderFlags;
pub use errors::ServiceError;
#[cfg(feature = "session")]
pub use errors::SubscriptionAckError;
#[cfg(feature = "native")]
pub use service::{KIND, Service, ServiceBuilder, ServiceConfiguration};
```

#### 4.9.3 `src/errors.rs` — gate native errors

```rust
#[cfg(feature = "native")]
#[error("grpc configuration error")]
GrpcConfigError(#[from] slim_config::grpc::errors::ConfigError),
```

---

### 4.10 `agntcy-slim-wasm` (`core/slim-wasm`)

This is a **new crate** — the browser entry point.

#### 4.10.1 `Cargo.toml`

```toml
[package]
name = "agntcy-slim-wasm"
edition = { workspace = true }
license = { workspace = true }
version = "0.1.0"
description = "WASM/browser entry point for SLIM data plane."

[lib]
name = "slim_wasm"
crate-type = ["cdylib", "rlib"]

[dependencies]
agntcy-slim-auth = { workspace = true, default-features = false, features = ["wasm"] }
agntcy-slim-config = { workspace = true, default-features = false, features = ["wasm"] }
agntcy-slim-datapath = { workspace = true, default-features = false, features = ["wasm"] }
agntcy-slim-session = { workspace = true, default-features = false, features = ["wasm"] }
agntcy-slim-mls = { workspace = true, default-features = false, features = ["wasm"] }
agntcy-slim-tracing = { workspace = true, default-features = false, features = ["wasm"] }

futures = { workspace = true }
js-sys = "0.3"
parking_lot = { workspace = true }
prost = { workspace = true }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
tracing = { workspace = true }
url = { workspace = true }
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
console_error_panic_hook = "0.1"

[target.'cfg(target_arch = "wasm32")'.dependencies]
gloo-net = "0.6"
```

#### 4.10.2 `src/lib.rs` — complete API

The file is ~714 lines. Key structure:

**Top-level function:**
```rust
#[wasm_bindgen(js_name = "initTracing")]
pub fn init_tracing() {
    console_error_panic_hook::set_once();
    let config = TracingConfiguration::default();
    let _ = config.setup_tracing_subscriber();
}
```

**`SlimClient` struct** (inside `#[cfg(target_arch = "wasm32")] mod wasm_impl`):

Fields:
- `app_name: Name`
- `session_layer: Arc<SessionLayer<SharedSecret, SharedSecret, AppTransmitter>>`
- `tx_slim: SlimChannelSender`
- `sessions: Arc<Mutex<HashMap<u32, Arc<SessionController>>>>`
- `notification_rx: Arc<Mutex<Option<Receiver<Result<Notification, SessionError>>>>>`
- `subscription_manager: SubscriptionManager`
- `on_message_cb: Arc<Mutex<Option<JsCallback>>>`
- `cancel_token: CancellationToken`

**`JsCallback` wrapper** — wraps `js_sys::Function` with unsafe `Send + Sync` (safe because WASM is single-threaded).

**Methods (all `#[wasm_bindgen]`):**

| Method | Signature | Description |
|--------|-----------|-------------|
| `connect` | `async (endpoint, shared_secret, org, ns, app) -> Result<SlimClient>` | Parse URL, append `?token=...`, open gloo-net WS, split sink/stream, build SessionLayer + AppTransmitter + identity interceptor, self-subscribe, spawn read loop |
| `subscribe` | `async (org, ns, name) -> Result<()>` | Use SubscriptionManager, await ack |
| `unsubscribe` | `async (org, ns, name) -> Result<()>` | Use SubscriptionManager, await ack |
| `createSession` | `async (dest_org, dest_ns, dest_app, session_type, mls_enabled?) -> Result<u32>` | Map session type string, create SessionConfig, call session_layer.create_session, store controller, spawn receiver task, await init ack |
| `listen` | `(on_message: Function, on_session: Function) -> Result<()>` | Store callbacks, take notification_rx, spawn loop handling NewMessage and NewSession notifications |
| `publish` | `async (session_id, payload: &[u8], payload_type?) -> Result<()>` | Get session by ID, call publish, await ack |
| `inviteParticipant` | `async (session_id, org, ns, name) -> Result<()>` | Get session, call invite_participant, await ack |
| `removeParticipant` | `async (session_id, org, ns, name) -> Result<()>` | Get session, call remove_participant, await ack |
| `participantsList` | `async (session_id) -> Result<Vec<String>>` | Get session, call participants_list |
| `deleteSession` | `(session_id) -> Result<()>` | Remove from map, remove from session layer |
| `disconnect` | `()` | Cancel token |
| `appName` | `getter -> String` | Format app_name |
| `sessionIds` | `() -> Vec<u32>` | Keys of sessions map |

**Helper functions:**
- `route_incoming_message(msg, session_layer, subscription_manager)` — handle subscription acks inline (synchronously), spawn all other messages into separate tasks to avoid deadlocking read loop
- `build_message_js_object(msg) -> JsValue` — creates JS object with `source`, `sessionId`, `payload` (Uint8Array), `payloadType`

**Read loop** (spawned in `connect`):
1. Read binary frames from WS stream
2. Decode as protobuf `Message`
3. Pass to `route_incoming_message`
4. Text frames ignored
5. Errors break the loop

**Write loop** (spawned in `connect`):
1. Receive from `rx_slim` channel
2. Encode as protobuf bytes
3. Send as binary WS frame via sink
4. Respects cancel token

---

## 5. Config YAML Samples

Create under `data-plane/config/websocket/`:

**`server-config.yaml`** (debug, ws://):
```yaml
tracing:
  log_level: debug
  display_thread_names: true
  display_thread_ids: true
runtime:
  n_cores: 0
  thread_name: "slim-data-plane"
  drain_timeout: 10s
services:
  slim/0:
    dataplane:
      servers:
        - endpoint: "0.0.0.0:46357"
          transport: websocket
          websocket_auth_query_param: token
          tls:
            insecure: true
```

**`client-config-debug.yaml`**:
```yaml
tracing:
  log_level: debug
  display_thread_names: true
  display_thread_ids: true
runtime:
  n_cores: 0
  thread_name: "slim-data-plane"
  drain_timeout: 10s
services:
  slim/0:
    dataplane:
      clients:
        - endpoint: "ws://localhost:46357"
          transport: websocket
          websocket_auth_query_param: token
          tls:
            insecure: true
      servers: []
```

Also create `server-config-wss.yaml` and `client-config-wss.yaml` with TLS cert/key paths and `wss://` endpoints.

---

## 6. CI / Automation

### 6.1 GitHub Actions — `.github/workflows/data-plane.yaml`

Add job:

```yaml
data-plane-wasm-check:
  name: Data plane - WASM build check
  runs-on: ubuntu-latest
  defaults:
    run:
      shell: bash
      working-directory: ./data-plane
  steps:
    - name: Checkout Repo
      uses: actions/checkout@v4
    - name: Setup Rust
      uses: ./.github/actions/setup-rust
      with:
        workspace: ./data-plane
    - name: Install wasm32-unknown-unknown target
      run: rustup target add wasm32-unknown-unknown
    - name: Check WASM compilation
      run: task data-plane:check:wasm
```

### 6.2 Taskfile — `data-plane/Taskfile.yaml`

Add tasks:

```yaml
data-plane:check:config-wasm:
  desc: "Check agntcy-slim-config compiles with wasm feature for wasm target"
  cmds:
    - cargo check --package agntcy-slim-config --no-default-features --features wasm --target wasm32-unknown-unknown --locked

data-plane:check:wasm:
  desc: "Check all WASM-ready crates compile for wasm32-unknown-unknown"
  cmds:
    - task: data-plane:check:config-wasm
    - cargo check -p agntcy-slim-auth --target wasm32-unknown-unknown --no-default-features --features wasm --locked
    - cargo check -p agntcy-slim-datapath --target wasm32-unknown-unknown --no-default-features --features wasm --locked
    - cargo check -p agntcy-slim-tracing --target wasm32-unknown-unknown --no-default-features --features wasm --locked
    - cargo check -p agntcy-slim-signal --target wasm32-unknown-unknown --no-default-features --features wasm --locked
    - cargo check -p agntcy-slim-mls --target wasm32-unknown-unknown --no-default-features --features wasm --locked
    - cargo check -p agntcy-slim-session --target wasm32-unknown-unknown --no-default-features --features wasm --locked
    - cargo check -p agntcy-slim-controller --target wasm32-unknown-unknown --no-default-features --features wasm --locked
    - cargo check -p agntcy-slim-service --target wasm32-unknown-unknown --no-default-features --features "wasm,session" --locked
```

### 6.3 Optional: `.github/agents/rust-wasm.agent.md`

Copilot/Cursor agent doc for future WASM work. See the patch for the full content.

---

## 7. Examples & Browser Demo

### 7.1 `data-plane/examples/Taskfile.yaml`

Add tasks for WebSocket transport:
- `run:slim:websocket` — runs slim with `config/websocket/server-config.yaml`
- `run:slim:websocket:wss` — runs slim with `config/websocket/server-config-wss.yaml`
- `run:mock-app:server-websocket` — mock app server over ws
- `run:mock-app:client-websocket` — mock app client over ws
- `run:mock-app:server-websocket-wss` / `client-websocket-wss` — wss variants
- `run:browser:serve` — `python3 -m http.server 8080`
- `run:browser` — `wasm-pack build core/slim-wasm --target web --out-dir ../../pkg`, then serve

### 7.2 `data-plane/examples/browser/index.html`

~403 lines. Dark-themed UI with:
- Connection card (endpoint, shared secret, org/ns/app fields)
- Subscribe/Unsubscribe section
- Session creation (P2P / multicast, MLS toggle)
- Publish section
- Participant management (invite/remove/list)
- Log panel with color-coded entries

Loads WASM via `import init, { initTracing, SlimClient } from "/pkg/slim_wasm.js"`.

### 7.3 `data-plane/examples/src/sdk-mock/args.rs`

Add `--mls-group-id` CLI argument.

### 7.4 `data-plane/examples/src/sdk-mock/main.rs`

Update to support WebSocket transport configs (the config file determines transport, no code change needed beyond ensuring the example works with both transport types).

---

## 8. Verification Commands

From `data-plane/`:

```bash
# Native (must still pass — no regressions)
cargo check -p agntcy-slim-service --features native,session
cargo test --workspace --locked

# WASM matrix
cargo check -p agntcy-slim-config --no-default-features --features wasm --target wasm32-unknown-unknown --locked
cargo check -p agntcy-slim-auth --no-default-features --features wasm --target wasm32-unknown-unknown --locked
cargo check -p agntcy-slim-datapath --no-default-features --features wasm --target wasm32-unknown-unknown --locked
cargo check -p agntcy-slim-tracing --no-default-features --features wasm --target wasm32-unknown-unknown --locked
cargo check -p agntcy-slim-signal --no-default-features --features wasm --target wasm32-unknown-unknown --locked
cargo check -p agntcy-slim-mls --no-default-features --features wasm --target wasm32-unknown-unknown --locked
cargo check -p agntcy-slim-session --no-default-features --features wasm --target wasm32-unknown-unknown --locked
cargo check -p agntcy-slim-controller --no-default-features --features wasm --target wasm32-unknown-unknown --locked
cargo check -p agntcy-slim-service --no-default-features --features wasm,session --target wasm32-unknown-unknown --locked

# Taskfile shortcut
task data-plane:check:wasm
```

---

## 9. Known WASM Limitations

| Limitation | Detail |
|-----------|--------|
| No WebSocket server | Browser can only act as WS client |
| No custom WS headers | Auth token sent via query param (`?token=xxx`) |
| Bounded channels ignored | WASM `channel(buffer)` creates unbounded (no backpressure) |
| JoinHandle fire-and-forget | WASM `spawn()` handle; awaiting it never resolves |
| Timer precision | Browser may throttle setTimeout in background tabs; durations >1 day treated as infinite |
| No gRPC | tonic/HTTP2 unavailable in browser |
| No SPIRE/mTLS | Unix sockets, certificate management are native-only |
| No file I/O | Config loading, cert files, file watchers are native-only |

---

## Appendix: File Inventory

Files to **create** (new):
- `data-plane/core/slim-wasm/Cargo.toml`
- `data-plane/core/slim-wasm/src/lib.rs`
- `data-plane/core/config/src/client.rs`
- `data-plane/core/config/src/server.rs`
- `data-plane/core/config/src/tls.rs`
- `data-plane/core/config/src/tls/client.rs`
- `data-plane/core/config/src/tls/server.rs`
- `data-plane/core/config/src/websocket.rs`
- `data-plane/core/config/src/websocket/client.rs`
- `data-plane/core/config/src/websocket/client_wasm.rs`
- `data-plane/core/config/src/websocket/common.rs`
- `data-plane/core/config/src/websocket/common_wasm.rs`
- `data-plane/core/config/src/websocket/server.rs`
- `data-plane/core/session/src/runtime.rs`
- `data-plane/core/tracing/src/native.rs`
- `data-plane/core/tracing/src/wasm.rs`
- `data-plane/core/tracing/src/utils.rs`
- `data-plane/core/mls/src/crypto.rs`
- `data-plane/core/datapath/src/websocket.rs`
- `data-plane/core/datapath/src/websocket/stream.rs`
- `data-plane/config/websocket/*.yaml` (4 sample configs)
- `data-plane/examples/browser/index.html`
- `data-plane/examples/Taskfile.yaml`
- `.github/agents/rust-wasm.agent.md` (optional)

Files to **modify**:
- `data-plane/Cargo.toml` (workspace members + deps)
- `data-plane/.cargo/config.toml` (wasm32 rustflags)
- `data-plane/Taskfile.yaml` (wasm check tasks)
- `.github/workflows/data-plane.yaml` (wasm CI job)
- All `Cargo.toml` files for crates listed in section 4
- All `src/lib.rs` files for crates listed in section 4
- `build.rs` in datapath and controller
- Session crate: ~20 source files (see section 4.6.5)
- Auth crate: `errors.rs`, `shared_secret.rs`, `traits.rs`, `lib.rs`
- Datapath: `lib.rs`, `api.rs`, `api/proto.rs`, `errors.rs`, `message_processing.rs`
- Controller: `lib.rs`, `errors.rs`, `config.rs`, `service.rs`
- Service: `lib.rs`, `errors.rs`, `service.rs`
- Signal: `lib.rs`
- Tracing: `lib.rs` (refactored into native.rs + wasm.rs)
- MLS: `lib.rs`, `mls.rs`, `identity_provider.rs`
- Examples: `sdk-mock/args.rs`, `sdk-mock/main.rs`
