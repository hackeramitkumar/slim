// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use base64::Engine;
use base64::engine::general_purpose::STANDARD as STANDARD_BASE64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use std::sync::Arc;

use crate::errors::AuthError;
use crate::traits::{TokenProvider, Verifier};
use crate::utils::generate_mls_signature_keys;

const MIN_SECRET_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const DEFAULT_VALIDITY_WINDOW: u64 = 3600;
const DEFAULT_CLOCK_SKEW: u64 = 5;

#[derive(Debug)]
struct SharedSecretInternal {
    base_id: String,
    id: String,
    shared_secret: String,
    validity_window: std::time::Duration,
    clock_skew: std::time::Duration,
}

#[derive(Clone)]
pub struct SharedSecret {
    inner: Arc<SharedSecretInternal>,
    signature_keys: (Vec<u8>, Vec<u8>),
}

impl std::fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedSecret")
            .field("base_id", &self.inner.base_id)
            .field("id", &self.inner.id)
            .field("has_signature_keys", &true)
            .finish()
    }
}

impl SharedSecret {
    pub fn new(id: &str, shared_secret: &str) -> Result<Self, AuthError> {
        Self::validate_id(id)?;
        Self::validate_secret(shared_secret)?;

        let random_suffix: String = {
            use rand::Rng;
            rand::rng()
                .sample_iter(&rand::distr::Alphanumeric)
                .take(8)
                .map(char::from)
                .collect()
        };
        let full_id = format!("{}_{}", id, random_suffix);

        let signature_keys = generate_mls_signature_keys().unwrap_or_else(|_| (vec![], vec![]));
        let internal = SharedSecretInternal {
            base_id: id.to_owned(),
            id: full_id,
            shared_secret: shared_secret.to_owned(),
            validity_window: std::time::Duration::from_secs(DEFAULT_VALIDITY_WINDOW),
            clock_skew: std::time::Duration::from_secs(DEFAULT_CLOCK_SKEW),
        };
        Ok(SharedSecret {
            inner: Arc::new(internal),
            signature_keys,
        })
    }

    pub fn id(&self) -> &str {
        &self.inner.id
    }

    pub fn base_id(&self) -> &str {
        &self.inner.base_id
    }

    pub fn validity_window_secs(&self) -> u64 {
        self.inner.validity_window.as_secs()
    }

    fn validate_id(id: &str) -> Result<(), AuthError> {
        if id.is_empty() || id.contains(':') || id.chars().any(|c| c.is_whitespace()) {
            return Err(AuthError::TokenMalformed);
        }
        Ok(())
    }

    fn validate_secret(secret: &str) -> Result<(), AuthError> {
        if secret.len() < MIN_SECRET_LEN {
            return Err(AuthError::HmacKeyTooShort);
        }
        Ok(())
    }

    fn get_current_timestamp(&self) -> u64 {
        current_timestamp_secs()
    }

    fn create_hmac_b64(&self, message: &str) -> Result<String, AuthError> {
        let raw = hmac_sign(self.inner.shared_secret.as_bytes(), message.as_bytes());
        Ok(URL_SAFE_NO_PAD.encode(raw))
    }

    fn verify_hmac(&self, message: &str, expected_b64: &str) -> Result<(), AuthError> {
        let expected = URL_SAFE_NO_PAD
            .decode(expected_b64.as_bytes())
            .map_err(|e| AuthError::Base64DecodeError(e))?;
        if expected.len() != 32 {
            return Err(AuthError::TokenMalformed);
        }
        let computed = hmac_sign(self.inner.shared_secret.as_bytes(), message.as_bytes());
        if !constant_time_eq(&computed, &expected) {
            return Err(AuthError::TokenInvalid);
        }
        Ok(())
    }

    fn build_message(&self, id: &str, timestamp: u64, nonce: &str, claims_b64: &str) -> String {
        format!("{}:{}:{}:{}", id, timestamp, nonce, claims_b64)
    }

    fn gen_nonce(&self) -> String {
        let mut bytes = [0u8; NONCE_LEN];
        {
            use rand::Rng;
            rand::rng().fill(&mut bytes);
        }
        URL_SAFE_NO_PAD.encode(bytes)
    }

    fn parse_token(&self, token: &str) -> Result<(String, u64, String, String, String), AuthError> {
        let parts: Vec<&str> = token.split(':').collect();
        if parts.len() != 5 {
            return Err(AuthError::TokenMalformed);
        }
        let id = parts[0].to_string();
        let ts = parts[1]
            .parse::<u64>()
            .map_err(|_| AuthError::TokenMalformed)?;
        let nonce = parts[2].to_string();
        let claims_b64 = parts[3].to_string();
        let mac = parts[4].to_string();
        Ok((id, ts, nonce, claims_b64, mac))
    }

    fn validate_timestamp(&self, now: u64, ts: u64) -> Result<(), AuthError> {
        if ts > now {
            let diff = ts - now;
            if diff > self.inner.clock_skew.as_secs() {
                return Err(AuthError::TokenInvalid);
            }
        } else {
            let age = now - ts;
            if age > self.inner.validity_window.as_secs() {
                return Err(AuthError::TokenInvalid);
            }
        }
        Ok(())
    }

    pub fn try_verify(&self, token: impl Into<String>) -> Result<(), AuthError> {
        let token_str = token.into();
        let now = self.get_current_timestamp();
        let (token_id, ts, _nonce, claims_b64, mac_b64) = self.parse_token(&token_str)?;
        self.validate_timestamp(now, ts)?;
        let message = self.build_message(&token_id, ts, &_nonce, &claims_b64);
        self.verify_hmac(&message, &mac_b64)
    }
}

#[cfg_attr(feature = "native", async_trait::async_trait)]
#[cfg_attr(feature = "wasm", async_trait::async_trait(?Send))]
impl TokenProvider for SharedSecret {
    fn get_token(&self) -> Result<String, AuthError> {
        if self.inner.shared_secret.is_empty() {
            return Err(AuthError::HmacKeyMissing);
        }
        let ts = self.get_current_timestamp();
        let nonce = self.gen_nonce();
        let claims_json = if self.signature_keys.1.is_empty() {
            serde_json::json!({}).to_string()
        } else {
            let pub_key_b64 = STANDARD_BASE64.encode(&self.signature_keys.1);
            serde_json::json!({"pubkey": pub_key_b64}).to_string()
        };
        let claims_b64 = URL_SAFE_NO_PAD.encode(claims_json.as_bytes());
        let message = self.build_message(self.id(), ts, &nonce, &claims_b64);
        let mac = self.create_hmac_b64(&message)?;
        Ok(format!(
            "{}:{}:{}:{}:{}",
            self.id(),
            ts,
            nonce,
            claims_b64,
            mac
        ))
    }

    fn get_id(&self) -> Result<String, AuthError> {
        Ok(self.id().to_string())
    }

    fn get_signature_secret_key(&self) -> Result<Vec<u8>, AuthError> {
        Ok(self.signature_keys.0.clone())
    }

    fn get_signature_public_key(&self) -> Result<Vec<u8>, AuthError> {
        Ok(self.signature_keys.1.clone())
    }

    fn rotate_signature_keys(&mut self) -> Result<(), AuthError> {
        self.signature_keys = generate_mls_signature_keys()?;
        Ok(())
    }

    fn set_signature_keys(&mut self, secret: Vec<u8>, public: Vec<u8>) -> Result<(), AuthError> {
        self.signature_keys = (secret, public);
        Ok(())
    }
}

#[cfg_attr(feature = "native", async_trait::async_trait)]
#[cfg_attr(feature = "wasm", async_trait::async_trait(?Send))]
impl Verifier for SharedSecret {
    fn try_get_claims<Claims>(&self, token: impl Into<String>) -> Result<Claims, AuthError>
    where
        Claims: serde::de::DeserializeOwned,
    {
        let token_str = token.into();
        self.try_verify(token_str.clone())?;
        let (token_id, ts, _, claims_b64, _) = self.parse_token(&token_str)?;
        let exp = ts + self.inner.validity_window.as_secs();

        let custom_claims: serde_json::Value = if !claims_b64.is_empty() {
            let claims_json = URL_SAFE_NO_PAD
                .decode(claims_b64.as_bytes())
                .map_err(|e| AuthError::Base64DecodeError(e))?;
            serde_json::from_slice(&claims_json)?
        } else {
            serde_json::json!({})
        };

        let claims_json = serde_json::json!({
            "sub": token_id,
            "iat": ts,
            "exp": exp,
            "custom_claims": custom_claims
        });

        let ret = serde_json::from_value(claims_json)?;
        Ok(ret)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(not(target_arch = "wasm32"))]
fn current_timestamp_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(target_arch = "wasm32")]
fn current_timestamp_secs() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

#[cfg(feature = "native")]
fn hmac_sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    use aws_lc_rs::hmac;
    let s_key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::sign(&s_key, data).as_ref().to_vec()
}

#[cfg(all(feature = "wasm", not(feature = "native")))]
fn hmac_sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_secret() -> String {
        "a".repeat(MIN_SECRET_LEN)
    }

    #[test]
    fn test_token_roundtrip() {
        let s = SharedSecret::new("svc", &valid_secret()).unwrap();
        let token = s.get_token().unwrap();
        assert!(s.try_verify(&token).is_ok());
    }

    #[test]
    fn test_cross_instance_verification() {
        let a = SharedSecret::new("svc", &valid_secret()).unwrap();
        let b = SharedSecret::new("svc", &valid_secret()).unwrap();
        let token = a.get_token().unwrap();
        assert!(b.try_verify(token).is_ok());
    }

    #[test]
    fn test_claims_extraction() {
        let s = SharedSecret::new("svc", &valid_secret()).unwrap();
        let token = s.get_token().unwrap();
        let claims: serde_json::Value = s.try_get_claims(token).unwrap();
        assert!(claims.get("sub").is_some());
        assert!(claims.get("custom_claims").is_some());
    }

    #[test]
    fn test_short_secret_rejected() {
        let result = SharedSecret::new("svc", "short");
        assert!(result.is_err());
    }
}
