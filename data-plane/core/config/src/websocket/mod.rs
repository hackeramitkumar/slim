// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

#[cfg(feature = "native")]
#[path = "client.rs"]
pub mod client;

#[cfg(all(feature = "wasm", not(feature = "native")))]
#[path = "client_wasm.rs"]
pub mod client;

#[cfg(feature = "native")]
pub mod common;

#[cfg(all(feature = "wasm", not(feature = "native")))]
#[path = "common_wasm.rs"]
pub mod common;

#[cfg(feature = "native")]
pub mod server;
