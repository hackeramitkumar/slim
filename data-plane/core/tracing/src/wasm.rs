// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use std::io::Write;

use thiserror::Error;
use tracing::Level;
use tracing_subscriber::{filter::LevelFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("tracing setup error: {0}")]
    SetupError(String),
}

struct ConsoleWriter {
    buf: Vec<u8>,
}

impl ConsoleWriter {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }
}

impl Write for ConsoleWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            let s = String::from_utf8_lossy(&self.buf);
            let s = s.trim_end();
            if !s.is_empty() {
                web_sys::console::log_1(&s.into());
            }
            self.buf.clear();
        }
        Ok(())
    }
}

impl Drop for ConsoleWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

struct ConsoleMakeWriter;

impl<'a> fmt::MakeWriter<'a> for ConsoleMakeWriter {
    type Writer = ConsoleWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleWriter::new()
    }
}

#[derive(Clone, Debug)]
pub struct TracingConfiguration {
    log_level: String,
}

impl Default for TracingConfiguration {
    fn default() -> Self {
        TracingConfiguration {
            log_level: "info".to_string(),
        }
    }
}

pub struct OtelGuard;

impl Drop for OtelGuard {
    fn drop(&mut self) {}
}

fn resolve_level(level: &str) -> Level {
    match level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    }
}

impl TracingConfiguration {
    pub fn with_log_level(self, log_level: String) -> Self {
        TracingConfiguration { log_level, ..self }
    }

    pub fn log_level(&self) -> &str {
        &self.log_level
    }

    pub fn setup_tracing_subscriber(&self) -> Result<OtelGuard, ConfigError> {
        let level_filter = LevelFilter::from_level(resolve_level(&self.log_level));

        let fmt_layer = fmt::layer()
            .with_writer(ConsoleMakeWriter)
            .with_ansi(false)
            .without_time();

        tracing_subscriber::registry()
            .with(level_filter)
            .with(fmt_layer)
            .init();

        Ok(OtelGuard)
    }
}
