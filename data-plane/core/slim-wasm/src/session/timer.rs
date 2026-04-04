// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

use futures::future::{AbortHandle, Abortable};
use tokio_with_wasm::alias as tokio;

#[derive(Debug, Clone)]
pub enum TimerType {
    Constant,
    Exponential,
}

pub struct TimerEvent {
    pub session_id: u32,
    pub timer_id: u32,
    pub kind: TimerEventKind,
}

pub enum TimerEventKind {
    Timeout,
    Failure,
}

pub struct Timer {
    timer_id: u32,
    timer_type: TimerType,
    duration: Duration,
    max_duration: Option<Duration>,
    max_retries: Option<u32>,
    abort_handle: Option<AbortHandle>,
}

impl Timer {
    pub fn new(
        timer_id: u32,
        timer_type: TimerType,
        duration: Duration,
        max_duration: Option<Duration>,
        max_retries: Option<u32>,
    ) -> Self {
        Timer {
            timer_id,
            timer_type,
            duration,
            max_duration,
            max_retries,
            abort_handle: None,
        }
    }

    pub fn start(&mut self, session_id: u32, event_tx: tokio::sync::mpsc::Sender<TimerEvent>) {
        self.stop();

        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        self.abort_handle = Some(abort_handle);

        let timer_id = self.timer_id;
        let timer_type = self.timer_type.clone();
        let duration = self.duration;
        let max_retries = self.max_retries;
        let max_duration = self.max_duration;

        let fut = async move {
            let mut retry = 0u32;
            let mut timeouts = 0u32;
            let mut last_duration = duration;

            loop {
                let sleep_duration = match timer_type {
                    TimerType::Constant => duration,
                    TimerType::Exponential => {
                        let d = if timeouts == 0 {
                            duration
                        } else {
                            last_duration.checked_mul(2).unwrap_or(last_duration)
                        };
                        match max_duration {
                            Some(max_d) if d > max_d => {
                                last_duration = max_d;
                                max_d
                            }
                            _ => {
                                last_duration = d;
                                d
                            }
                        }
                    }
                };

                tokio_with_wasm::time::sleep(sleep_duration).await;
                timeouts += 1;

                match max_retries {
                    Some(max) if retry >= max => {
                        let _ = event_tx
                            .send(TimerEvent {
                                session_id,
                                timer_id,
                                kind: TimerEventKind::Failure,
                            })
                            .await;
                        break;
                    }
                    _ => {
                        let _ = event_tx
                            .send(TimerEvent {
                                session_id,
                                timer_id,
                                kind: TimerEventKind::Timeout,
                            })
                            .await;
                    }
                }
                retry += 1;
            }
        };

        let abortable = Abortable::new(fut, abort_registration);
        tokio_with_wasm::spawn(async move {
            let _ = abortable.await;
        });
    }

    pub fn stop(&mut self) {
        if let Some(handle) = self.abort_handle.take() {
            handle.abort();
        }
    }

    pub fn reset(&mut self, session_id: u32, event_tx: tokio::sync::mpsc::Sender<TimerEvent>) {
        self.stop();
        self.start(session_id, event_tx);
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.stop();
    }
}
