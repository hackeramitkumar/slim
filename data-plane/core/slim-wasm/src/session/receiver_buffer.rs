// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashSet;

use slim_datapath::api::proto::pubsub::v1::Message;
use tracing::debug;

pub struct ReceiverBuffer {
    last_sent: usize,
    first_entry: usize,
    lost_msgs: HashSet<usize>,
    buffer: Vec<Option<Message>>,
}

impl Default for ReceiverBuffer {
    fn default() -> Self {
        ReceiverBuffer {
            last_sent: usize::MAX,
            first_entry: 0,
            lost_msgs: HashSet::new(),
            buffer: vec![],
        }
    }
}

impl ReceiverBuffer {
    pub fn on_received_message(&mut self, msg: Message) -> (Vec<Option<Message>>, Vec<u32>) {
        self.internal_on_received_message(msg.get_id() as usize, Some(msg))
    }

    pub fn on_lost_message(&mut self, msg_id: u32) -> Vec<Option<Message>> {
        debug!("message {} is definitely lost", msg_id);
        self.lost_msgs.insert(msg_id as usize);
        self.release_msgs()
    }

    pub fn on_beacon_message(&mut self, msg_id: u32) -> Vec<u32> {
        debug!("received beacon for msg {}", msg_id);
        let (_recv, rtx) = self.internal_on_received_message(msg_id as usize, None);
        rtx
    }

    fn internal_on_received_message(
        &mut self,
        msg_id: usize,
        msg: Option<Message>,
    ) -> (Vec<Option<Message>>, Vec<u32>) {
        if self.last_sent == usize::MAX
            || (msg_id == (self.last_sent + 1)) && (self.buffer.is_empty())
        {
            match msg {
                Some(m) => {
                    self.last_sent = msg_id;
                    return (vec![Some(m)], vec![]);
                }
                None => {
                    return (vec![], vec![msg_id as u32]);
                }
            }
        }

        if msg_id <= self.last_sent {
            return (vec![], vec![]);
        }

        if self.buffer.is_empty() {
            self.first_entry = 0;
            let mut rtx: Vec<u32> = Vec::new();
            match msg {
                Some(m) => {
                    self.buffer = vec![None; msg_id - (self.last_sent + 1)];
                    self.buffer.push(Some(m));
                    for i in (self.last_sent + 1)..msg_id {
                        rtx.push(i as u32);
                    }
                }
                None => {
                    self.buffer = vec![None; msg_id - (self.last_sent + 1) + 1];
                    for i in (self.last_sent + 1)..=msg_id {
                        rtx.push(i as u32);
                    }
                }
            }
            (vec![], rtx)
        } else {
            if msg_id <= (self.last_sent + (self.buffer.len() - self.first_entry)) {
                if msg.is_none() {
                    return (vec![], vec![]);
                }

                let pos = msg_id - (self.last_sent + 1) + self.first_entry;
                if self.buffer[pos].is_some() {
                    return (vec![], vec![]);
                }
                self.buffer[pos] = msg;

                (self.release_msgs(), vec![])
            } else {
                let mut rtx = Vec::new();
                for i in ((self.last_sent + 1) + (self.buffer.len() - self.first_entry))..msg_id {
                    self.buffer.push(None);
                    rtx.push(i as u32);
                }
                match msg {
                    Some(m) => {
                        self.buffer.push(Some(m));
                    }
                    None => {
                        rtx.push(msg_id as u32);
                    }
                }
                (vec![], rtx)
            }
        }
    }

    fn release_msgs(&mut self) -> Vec<Option<Message>> {
        let mut i = self.first_entry;
        let mut ret = vec![];
        while i < self.buffer.len() {
            if self.buffer[i].is_some() {
                ret.push(self.buffer[i].take());
                self.last_sent += 1;
                self.first_entry += 1;
            } else if self.lost_msgs.contains(&(self.last_sent + 1)) {
                ret.push(None);
                self.lost_msgs.remove(&(self.last_sent + 1));
                self.last_sent += 1;
                self.first_entry += 1;
            } else {
                break;
            }
            i += 1;
        }
        if self.first_entry == self.buffer.len() {
            self.first_entry = 0;
            self.buffer = vec![];
        }
        let mut stop = false;
        while !stop {
            if self.lost_msgs.contains(&(self.last_sent + 1)) {
                self.last_sent += 1;
                ret.push(None);
                self.lost_msgs.remove(&self.last_sent);
            } else {
                stop = true;
            }
        }
        ret
    }
}
