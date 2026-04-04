// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

// JWT validation module - native only.
// Placeholder for JWT token validation using jsonwebtoken crate.

use crate::errors::AuthError;
use crate::identity_claims::IdentityClaims;

pub struct JwtValidator {
    // TODO: add JWT validation configuration
}

impl JwtValidator {
    pub fn new() -> Self {
        Self {}
    }

    pub fn validate(&self, _token: &str) -> Result<IdentityClaims, AuthError> {
        Err(AuthError::JwtError("JWT validation not yet implemented".into()))
    }
}
