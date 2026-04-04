// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

pub async fn shutdown() {
    imp::shutdown().await
}

#[cfg(all(feature = "native", unix))]
mod imp {
    use tokio::signal::unix::{SignalKind, signal};
    use tracing::info;

    pub(super) async fn shutdown() {
        tokio::select! {
            _ = sig(SignalKind::interrupt(), "SIGINT") => {}
            _ = sig(SignalKind::terminate(), "SIGTERM") => {}
        };
    }

    async fn sig(kind: SignalKind, name: &str) {
        signal(kind)
            .expect("Failed to register signal handler")
            .recv()
            .await;
        info!(
            target: "slim::signal",
            "received signal {}, starting shutdown",
            name,
        );
    }
}

#[cfg(all(feature = "native", not(unix)))]
mod imp {
    use tracing::info;

    pub(super) async fn shutdown() {
        tokio::signal::windows::ctrl_c()
            .expect("Failed to register signal handler")
            .recv()
            .await;
        info!(
            target: "slim::signal",
            "received signal Ctrl-C, starting shutdown",
        );
    }
}

#[cfg(all(feature = "wasm", not(feature = "native")))]
mod imp {
    pub(super) async fn shutdown() {
        std::future::pending::<()>().await
    }
}
