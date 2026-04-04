// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use slim_datapath::api::proto::pubsub::v1::Message;
use slim_datapath::messages::AgentType;

pub struct ProducerBuffer {
    capacity: usize,
    next: usize,
    buffer: Vec<Option<Message>>,
    map: HashMap<usize, usize>,
    destination_name: AgentType,
}

impl ProducerBuffer {
    pub fn with_capacity(capacity: usize) -> Self {
        ProducerBuffer {
            capacity,
            next: 0,
            buffer: vec![None; capacity],
            map: HashMap::new(),
            destination_name: AgentType::default(),
        }
    }

    pub fn get_destination_name(&self) -> &AgentType {
        &self.destination_name
    }

    pub fn push(&mut self, msg: Message) -> bool {
        if self.map.is_empty() {
            (self.destination_name, _) = msg.get_name();
        }

        let id = msg.get_id() as usize;

        if self.map.contains_key(&id) {
            return true;
        }

        if let Some(message) = &self.buffer[self.next] {
            let to_remove = message.get_id() as usize;
            self.map.remove(&to_remove);
        }

        self.buffer[self.next] = Some(msg);
        self.map.insert(id, self.next);
        self.next = (self.next + 1) % self.capacity;
        true
    }

    pub fn get(&self, id: usize) -> Option<Message> {
        match self.map.get(&id) {
            None => None,
            Some(index) => self.buffer[*index].clone(),
        }
    }
}
