// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

//! Platform-agnostic async runtime.
//!
//! Re-exports from `tokio_with_wasm` which provides a unified tokio-like API
//! that works on both native (via real tokio) and WASM (via browser JS glue).
//! This eliminates the need for manual `#[cfg]` branching on spawn, sleep,
//! channels, select, etc.
//!
//! Usage: `use slim_session::runtime::tokio;` then use `tokio::spawn(...)`,
//! `tokio::sync::mpsc::channel(...)`, `tokio::time::sleep(...)`, etc.

pub use tokio_with_wasm::alias as tokio;

pub use slim_datapath::Status;
pub use std::time::Duration;
