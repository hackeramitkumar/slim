// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

// File watcher module - native only.
// Watches credential files for changes and triggers reloads.

use crate::errors::AuthError;

pub struct FileWatcher {
    // TODO: implement credential file watching
}

impl FileWatcher {
    pub fn new(_path: &str) -> Result<Self, AuthError> {
        Ok(Self {})
    }
}
