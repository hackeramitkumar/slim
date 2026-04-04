// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value as JsonValue;

use crate::errors::AuthError;

pub mod claim_keys {
    pub const PUBKEY: &str = "pubkey";
    pub const SUBJECT: &str = "sub";
    pub const CUSTOM_CLAIMS: &str = "custom_claims";
}

#[derive(Debug, Clone)]
pub struct IdentityClaims {
    pub subject: String,
    pub public_key: String,
}

impl IdentityClaims {
    pub fn new(subject: impl Into<String>, public_key: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            public_key: public_key.into(),
        }
    }

    pub fn from_json(claims: &JsonValue) -> Result<Self, AuthError> {
        let public_key = claims
            .get(claim_keys::PUBKEY)
            .and_then(|pk| pk.as_str())
            .or_else(|| {
                claims
                    .get(claim_keys::CUSTOM_CLAIMS)
                    .and_then(|c| c.as_object())
                    .and_then(|cc| cc.get(claim_keys::PUBKEY))
                    .and_then(|pk| pk.as_str())
            })
            .ok_or(AuthError::PublicKeyNotFound)?;

        let subject = claims
            .get(claim_keys::SUBJECT)
            .and_then(|s| s.as_str())
            .or_else(|| claims.get("id").and_then(|s| s.as_str()))
            .ok_or(AuthError::SubjectNotFound)?;

        Ok(Self {
            subject: subject.to_string(),
            public_key: public_key.to_string(),
        })
    }

    pub fn encode_public_key(public_key_bytes: &[u8]) -> String {
        BASE64.encode(public_key_bytes)
    }
}

impl std::fmt::Display for IdentityClaims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.subject, self.public_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_from_json_top_level_claims() {
        let claims = json!({
            "sub": "user123",
            "pubkey": "base64encodedkey"
        });

        let identity_claims = IdentityClaims::from_json(&claims).unwrap();
        assert_eq!(identity_claims.subject, "user123");
        assert_eq!(identity_claims.public_key, "base64encodedkey");
    }

    #[test]
    fn test_from_json_custom_claims() {
        let claims = json!({
            "sub": "user123",
            "custom_claims": {
                "pubkey": "base64encodedkey"
            }
        });

        let identity_claims = IdentityClaims::from_json(&claims).unwrap();
        assert_eq!(identity_claims.subject, "user123");
        assert_eq!(identity_claims.public_key, "base64encodedkey");
    }

    #[test]
    fn test_from_json_missing_pubkey() {
        let claims = json!({
            "sub": "user123"
        });

        let result = IdentityClaims::from_json(&claims);
        assert!(matches!(result, Err(AuthError::PublicKeyNotFound)));
    }

    #[test]
    fn test_from_json_missing_subject() {
        let claims = json!({
            "pubkey": "base64encodedkey"
        });

        let result = IdentityClaims::from_json(&claims);
        assert!(matches!(result, Err(AuthError::SubjectNotFound)));
    }

    #[test]
    fn test_encode_public_key() {
        let public_key_bytes = b"test_public_key";
        let encoded = IdentityClaims::encode_public_key(public_key_bytes);
        assert!(!encoded.is_empty());
        assert_eq!(encoded, BASE64.encode(public_key_bytes));
    }
}
