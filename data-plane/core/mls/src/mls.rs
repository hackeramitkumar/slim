// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

use mls_rs::IdentityProvider;
use mls_rs::{
    CipherSuite, CipherSuiteProvider, Client, CryptoProvider, ExtensionList, Group, MlsMessage,
    crypto::{SignaturePublicKey, SignatureSecretKey},
    group::ReceivedMessage,
    identity::{SigningIdentity, basic::BasicCredential},
};
use mls_rs_core::identity::MemberValidationContext;

#[cfg(feature = "native")]
use mls_rs_crypto_awslc::AwsLcCryptoProvider;
#[cfg(feature = "wasm")]
use mls_rs_crypto_webcrypto::WebCryptoProvider;

use std::collections::HashSet;
use tracing::debug;

use slim_auth::traits::{TokenProvider, Verifier};

use crate::errors::MlsError;
use crate::identity_provider::SlimIdentityProvider;

#[cfg(feature = "native")]
const CIPHERSUITE: CipherSuite = CipherSuite::CURVE25519_AES128;
#[cfg(feature = "wasm")]
const CIPHERSUITE: CipherSuite = CipherSuite::P256_AES128;

#[cfg(feature = "native")]
type CryptoProviderT = AwsLcCryptoProvider;
#[cfg(feature = "wasm")]
type CryptoProviderT = WebCryptoProvider;

type MlsConfig<V> = mls_rs::client_builder::WithIdentityProvider<
    SlimIdentityProvider<V>,
    mls_rs::client_builder::WithCryptoProvider<CryptoProviderT, mls_rs::client_builder::BaseConfig>,
>;

pub type CommitMsg = Vec<u8>;
pub type WelcomeMsg = Vec<u8>;
pub type ProposalMsg = Vec<u8>;
pub type KeyPackageMsg = Vec<u8>;
pub type MlsIdentity = Vec<u8>;
pub struct MlsAddMemberResult {
    pub welcome_message: WelcomeMsg,
    pub commit_message: CommitMsg,
    pub member_identity: MlsIdentity,
}

#[derive(Clone, Debug)]
struct InMemoryIdentity {
    #[allow(dead_code)]
    identifier: String,
    public_key_bytes: Vec<u8>,
    private_key_bytes: Vec<u8>,
    last_credential: Option<String>,
    credential_version: u64,
}

pub struct Mls<P, V>
where
    P: TokenProvider + Send + Sync + Clone + 'static,
    V: Verifier + Send + Sync + Clone + 'static,
{
    identity: Option<String>,
    stored_identity: Option<InMemoryIdentity>,
    client: Option<Client<MlsConfig<V>>>,
    group: Option<Group<MlsConfig<V>>>,
    identity_provider: P,
    identity_verifier: V,
}

impl<P, V> std::fmt::Debug for Mls<P, V>
where
    P: TokenProvider + Send + Sync + Clone + 'static,
    V: Verifier + Send + Sync + Clone + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug_struct = f.debug_struct("mls");
        debug_struct
            .field("identity", &self.identity)
            .field("has_client", &self.client.is_some())
            .field("has_group", &self.group.is_some());

        if let Some(group) = &self.group {
            debug_struct
                .field("group_id", &hex::encode(group.group_id()))
                .field("epoch", &group.current_epoch());
        }

        debug_struct.finish()
    }
}

impl<P, V> Mls<P, V>
where
    P: TokenProvider + Send + Sync + Clone + 'static,
    V: Verifier + Send + Sync + Clone + 'static,
{
    pub fn new(identity_provider: P, identity_verifier: V) -> Self {
        Self {
            identity: None,
            stored_identity: None,
            client: None,
            group: None,
            identity_provider,
            identity_verifier,
        }
    }

    fn create_signing_identity(
        &mut self,
        is_rotation: bool,
    ) -> Result<(SignatureSecretKey, SigningIdentity), MlsError> {
        let token = self.identity_provider.get_token()?;
        let pub_key_bytes = self.identity_provider.get_signature_public_key()?;
        let priv_key_bytes = self.identity_provider.get_signature_secret_key()?;

        let public_key = SignaturePublicKey::new(pub_key_bytes.clone());
        let private_key = SignatureSecretKey::new(priv_key_bytes.clone());

        let basic_cred = BasicCredential::new(token.as_bytes().to_vec());
        let signing_identity = SigningIdentity::new(basic_cred.into_credential(), public_key);

        if let Some(stored) = self.stored_identity.as_mut() {
            stored.last_credential = Some(token);
            stored.public_key_bytes = pub_key_bytes;
            stored.private_key_bytes = priv_key_bytes;

            if is_rotation {
                stored.credential_version = stored.credential_version.saturating_add(1);
            }
        }

        Ok((private_key, signing_identity))
    }

    fn build_client(
        &self,
        signing_identity: SigningIdentity,
        private_key: SignatureSecretKey,
    ) -> Client<MlsConfig<V>> {
        let crypto_provider = CryptoProviderT::default();
        let identity_provider = SlimIdentityProvider::new(self.identity_verifier.clone());

        Client::builder()
            .identity_provider(identity_provider)
            .crypto_provider(crypto_provider)
            .signing_identity(signing_identity, private_key, CIPHERSUITE)
            .build()
    }

    pub fn get_group_id(&self) -> Option<Vec<u8>> {
        self.group.as_ref().map(|g| g.group_id().to_vec())
    }

    pub fn get_epoch(&self) -> Option<u64> {
        self.group.as_ref().map(|g| g.current_epoch())
    }

    pub fn get_token(&self) -> Result<String, MlsError> {
        let ret = self.identity_provider.get_token()?;
        Ok(ret)
    }
}

// ---------------------------------------------------------------------------
// Native (sync) implementation
// ---------------------------------------------------------------------------
#[cfg(not(mls_build_async))]
impl<P, V> Mls<P, V>
where
    P: TokenProvider + Send + Sync + Clone + 'static,
    V: Verifier + Send + Sync + Clone + 'static,
{
    #[cfg(test)]
    fn generate_key_pair() -> Result<(SignatureSecretKey, SignaturePublicKey), MlsError> {
        let crypto_provider = AwsLcCryptoProvider::default();
        let cipher_suite_provider = crypto_provider
            .cipher_suite_provider(CIPHERSUITE)
            .ok_or(MlsError::CiphersuiteUnavailable)?;

        cipher_suite_provider
            .signature_key_generate()
            .map_err(MlsError::crypto_provider)
    }

    pub fn initialize(&mut self) -> Result<(), MlsError> {
        debug!("Initializing MLS");

        self.identity_provider.rotate_signature_keys()?;

        self.identity = Some(self.identity_provider.get_id()?);

        let stored_identity = InMemoryIdentity {
            identifier: self
                .identity
                .clone()
                .map(|id| id.to_string())
                .expect("MLS identity could not be determined from identity provider"),
            public_key_bytes: vec![],
            private_key_bytes: vec![],
            last_credential: None,
            credential_version: 1,
        };

        self.stored_identity = Some(stored_identity);

        let (private_key, signing_identity) = self.create_signing_identity(false)?;

        self.client = Some(self.build_client(signing_identity, private_key));
        debug!("MLS client initialization completed successfully");
        Ok(())
    }

    pub fn create_group(&mut self) -> Result<Vec<u8>, MlsError> {
        debug!("Creating new MLS group");
        let client = self.client.as_ref().ok_or(MlsError::ClientNotInitialized)?;

        let group = client.create_group(ExtensionList::default(), Default::default(), None)?;

        let group_id = group.group_id().to_vec();
        self.group = Some(group);
        debug!(
            id = ?hex::encode(&group_id),
            "MLS group created successfully",
        );

        Ok(group_id)
    }

    pub fn generate_key_package(&self) -> Result<KeyPackageMsg, MlsError> {
        debug!("Generating key package");
        let client = self.client.as_ref().ok_or(MlsError::ClientNotInitialized)?;

        let key_package =
            client.generate_key_package_message(Default::default(), Default::default(), None)?;

        let ret = key_package.to_bytes()?;
        Ok(ret)
    }

    pub fn add_member(&mut self, key_package_bytes: &[u8]) -> Result<MlsAddMemberResult, MlsError> {
        debug!("Adding member to the MLS group");
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;
        let key_package = MlsMessage::from_bytes(key_package_bytes)?;

        let identity_provider = SlimIdentityProvider::new(self.identity_verifier.clone());

        let kp = key_package
            .as_key_package()
            .ok_or(MlsError::KeyPackageMissing)?;
        identity_provider
            .validate_member(
                kp.signing_identity(),
                None,
                MemberValidationContext::None,
            )
            .map_err(|e| {
                MlsError::KeyPackageCredentialRejected(format!(
                    "new member identity verification failed: {e}"
                ))
            })?;
        debug!("Key package credential validated successfully");

        let old_roster = group.roster().members();
        let mut ids = HashSet::new();
        for m in old_roster {
            let identifier = identity_provider.identity(&m.signing_identity, &m.extensions)?;
            ids.insert(identifier);
        }

        let commit = group.commit_builder().add_member(key_package)?;
        let commit = commit.build()?;

        let commit_msg = commit.commit_message.to_bytes()?;

        let welcome = commit
            .welcome_messages
            .first()
            .ok_or(MlsError::NoWelcomeMessage)
            .and_then(|w| w.to_bytes().map_err(MlsError::from))?;

        group.apply_pending_commit()?;

        let new_roster = group.roster().members();
        let mut new_id = vec![];
        for m in new_roster {
            let identifier = identity_provider.identity(&m.signing_identity, &m.extensions)?;
            if !ids.contains(&identifier) {
                new_id = identifier;
                break;
            }
        }

        let ret = MlsAddMemberResult {
            welcome_message: welcome,
            commit_message: commit_msg,
            member_identity: new_id,
        };
        Ok(ret)
    }

    pub fn remove_member(&mut self, identity: &[u8]) -> Result<CommitMsg, MlsError> {
        debug!("Removing member from the MLS group");
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let m = group.member_with_identity(identity)?;

        let commit = group.commit_builder().remove_member(m.index)?;
        let commit = commit.build()?;

        let commit_msg = commit.commit_message.to_bytes()?;

        group.apply_pending_commit()?;

        Ok(commit_msg)
    }

    pub fn process_commit(&mut self, commit_message: &[u8]) -> Result<(), MlsError> {
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;
        let commit = MlsMessage::from_bytes(commit_message)?;

        group.process_incoming_message(commit)?;
        Ok(())
    }

    pub fn process_welcome(&mut self, welcome_message: &[u8]) -> Result<Vec<u8>, MlsError> {
        debug!("Processing welcome message and joining MLS group");
        let client = self.client.as_ref().ok_or(MlsError::ClientNotInitialized)?;

        let welcome = MlsMessage::from_bytes(welcome_message)?;
        let (group, _) = client.join_group(None, &welcome, None)?;

        let group_id = group.group_id().to_vec();
        self.group = Some(group);
        debug!(
            id = %hex::encode(&group_id),
            "Successfully joined MLS group",
        );

        Ok(group_id)
    }

    pub fn process_proposal(
        &mut self,
        proposal_message: &[u8],
        create_commit: bool,
    ) -> Result<CommitMsg, MlsError> {
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;
        let proposal = MlsMessage::from_bytes(proposal_message)?;

        group.process_incoming_message(proposal)?;

        if !create_commit {
            debug!("process proposal but do not create commit. return empty commit");
            return Ok(vec![]);
        }

        let commit = group.commit_builder().build()?;

        group.apply_pending_commit()?;

        let commit_msg = commit.commit_message.to_bytes()?;
        Ok(commit_msg)
    }

    pub fn process_local_pending_proposal(&mut self) -> Result<CommitMsg, MlsError> {
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let commit = group.commit_builder().build()?;

        group.apply_pending_commit()?;

        let commit_msg = commit.commit_message.to_bytes()?;
        Ok(commit_msg)
    }

    pub fn encrypt_message(&mut self, message: &[u8]) -> Result<Vec<u8>, MlsError> {
        debug!("Encrypting MLS message");

        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let encrypted_msg = group.encrypt_application_message(message, Default::default())?;

        let msg = encrypted_msg.to_bytes()?;
        Ok(msg)
    }

    pub fn decrypt_message(&mut self, encrypted_message: &[u8]) -> Result<Vec<u8>, MlsError> {
        debug!("Decrypting MLS message");

        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let message = MlsMessage::from_bytes(encrypted_message)?;

        match group.process_incoming_message(message)? {
            ReceivedMessage::ApplicationMessage(app_msg) => Ok(app_msg.data().to_vec()),
            _ => Err(MlsError::verification_failed(
                "Message was not an application message",
            )),
        }
    }

    pub fn write_to_storage(&mut self) -> Result<(), MlsError> {
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;
        group.write_to_storage()?;
        Ok(())
    }

    pub fn create_rotation_proposal(&mut self) -> Result<ProposalMsg, MlsError> {
        self.identity_provider.rotate_signature_keys()?;

        let (new_private_key, new_signing_identity) = self.create_signing_identity(true)?;

        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let update_proposal = group.propose_update_with_identity(
            new_private_key.clone(),
            new_signing_identity,
            vec![],
        )?;

        debug!(
            "Created credential rotation proposal, stored new keys and incremented credential version"
        );

        let ret = update_proposal.to_bytes()?;
        Ok(ret)
    }
}

// ---------------------------------------------------------------------------
// WASM (async) implementation
// ---------------------------------------------------------------------------
#[cfg(mls_build_async)]
impl<P, V> Mls<P, V>
where
    P: TokenProvider + Send + Sync + Clone + 'static,
    V: Verifier + Send + Sync + Clone + 'static,
{
    pub async fn initialize(&mut self) -> Result<(), MlsError> {
        debug!("Initializing MLS (WASM)");

        let crypto_provider = WebCryptoProvider::default();
        let cipher_suite_provider = crypto_provider
            .cipher_suite_provider(CIPHERSUITE)
            .ok_or(MlsError::CiphersuiteUnavailable)?;

        let (secret_key, public_key) = cipher_suite_provider
            .signature_key_generate()
            .await
            .map_err(MlsError::crypto_provider)?;

        self.identity_provider.set_signature_keys(
            secret_key.as_ref().to_vec(),
            public_key.as_ref().to_vec(),
        )?;

        self.identity = Some(self.identity_provider.get_id()?);

        let stored_identity = InMemoryIdentity {
            identifier: self
                .identity
                .clone()
                .map(|id| id.to_string())
                .expect("MLS identity could not be determined from identity provider"),
            public_key_bytes: vec![],
            private_key_bytes: vec![],
            last_credential: None,
            credential_version: 1,
        };

        self.stored_identity = Some(stored_identity);

        let (private_key, signing_identity) = self.create_signing_identity(false)?;

        self.client = Some(self.build_client(signing_identity, private_key));
        debug!("MLS client initialization completed successfully");
        Ok(())
    }

    pub async fn create_group(&mut self) -> Result<Vec<u8>, MlsError> {
        debug!("Creating new MLS group");
        let client = self.client.as_ref().ok_or(MlsError::ClientNotInitialized)?;

        let group = client
            .create_group(ExtensionList::default(), Default::default(), None)
            .await?;

        let group_id = group.group_id().to_vec();
        self.group = Some(group);
        debug!(
            id = ?hex::encode(&group_id),
            "MLS group created successfully",
        );

        Ok(group_id)
    }

    pub async fn generate_key_package(&self) -> Result<KeyPackageMsg, MlsError> {
        debug!("Generating key package");
        let client = self.client.as_ref().ok_or(MlsError::ClientNotInitialized)?;

        let key_package = client
            .generate_key_package_message(Default::default(), Default::default(), None)
            .await?;

        let ret = key_package.to_bytes()?;
        Ok(ret)
    }

    pub async fn add_member(
        &mut self,
        key_package_bytes: &[u8],
    ) -> Result<MlsAddMemberResult, MlsError> {
        debug!("Adding member to the MLS group");
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;
        let key_package = MlsMessage::from_bytes(key_package_bytes)?;

        let identity_provider = SlimIdentityProvider::new(self.identity_verifier.clone());

        let kp = key_package
            .as_key_package()
            .ok_or(MlsError::KeyPackageMissing)?;
        identity_provider
            .validate_member(
                kp.signing_identity(),
                None,
                MemberValidationContext::None,
            )
            .await
            .map_err(|e| {
                MlsError::KeyPackageCredentialRejected(format!(
                    "new member identity verification failed: {e}"
                ))
            })?;
        debug!("Key package credential validated successfully");

        let old_roster = group.roster().members();
        let mut ids = HashSet::new();
        for m in old_roster {
            let identifier = identity_provider
                .identity(&m.signing_identity, &m.extensions)
                .await?;
            ids.insert(identifier);
        }

        let commit = group.commit_builder().add_member(key_package)?.build().await?;

        let commit_msg = commit.commit_message.to_bytes()?;

        let welcome = commit
            .welcome_messages
            .first()
            .ok_or(MlsError::NoWelcomeMessage)
            .and_then(|w| w.to_bytes().map_err(MlsError::from))?;

        group.apply_pending_commit().await?;

        let new_roster = group.roster().members();
        let mut new_id = vec![];
        for m in new_roster {
            let identifier = identity_provider
                .identity(&m.signing_identity, &m.extensions)
                .await?;
            if !ids.contains(&identifier) {
                new_id = identifier;
                break;
            }
        }

        let ret = MlsAddMemberResult {
            welcome_message: welcome,
            commit_message: commit_msg,
            member_identity: new_id,
        };
        Ok(ret)
    }

    pub async fn remove_member(&mut self, identity: &[u8]) -> Result<CommitMsg, MlsError> {
        debug!("Removing member from the MLS group");
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let m = group.member_with_identity(identity).await?;

        let commit = group
            .commit_builder()
            .remove_member(m.index)?
            .build()
            .await?;

        let commit_msg = commit.commit_message.to_bytes()?;

        group.apply_pending_commit().await?;

        Ok(commit_msg)
    }

    pub async fn process_commit(&mut self, commit_message: &[u8]) -> Result<(), MlsError> {
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;
        let commit = MlsMessage::from_bytes(commit_message)?;

        group.process_incoming_message(commit).await?;
        Ok(())
    }

    pub async fn process_welcome(&mut self, welcome_message: &[u8]) -> Result<Vec<u8>, MlsError> {
        debug!("Processing welcome message and joining MLS group");
        let client = self.client.as_ref().ok_or(MlsError::ClientNotInitialized)?;

        let welcome = MlsMessage::from_bytes(welcome_message)?;
        let (group, _) = client.join_group(None, &welcome, None).await?;

        let group_id = group.group_id().to_vec();
        self.group = Some(group);
        debug!(
            id = %hex::encode(&group_id),
            "Successfully joined MLS group",
        );

        Ok(group_id)
    }

    pub async fn process_proposal(
        &mut self,
        proposal_message: &[u8],
        create_commit: bool,
    ) -> Result<CommitMsg, MlsError> {
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;
        let proposal = MlsMessage::from_bytes(proposal_message)?;

        group.process_incoming_message(proposal).await?;

        if !create_commit {
            debug!("process proposal but do not create commit. return empty commit");
            return Ok(vec![]);
        }

        let commit = group.commit_builder().build().await?;

        group.apply_pending_commit().await?;

        let commit_msg = commit.commit_message.to_bytes()?;
        Ok(commit_msg)
    }

    pub async fn process_local_pending_proposal(&mut self) -> Result<CommitMsg, MlsError> {
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let commit = group.commit_builder().build().await?;

        group.apply_pending_commit().await?;

        let commit_msg = commit.commit_message.to_bytes()?;
        Ok(commit_msg)
    }

    pub async fn encrypt_message(&mut self, message: &[u8]) -> Result<Vec<u8>, MlsError> {
        debug!("Encrypting MLS message");

        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let encrypted_msg = group
            .encrypt_application_message(message, Default::default())
            .await?;

        let msg = encrypted_msg.to_bytes()?;
        Ok(msg)
    }

    pub async fn decrypt_message(
        &mut self,
        encrypted_message: &[u8],
    ) -> Result<Vec<u8>, MlsError> {
        debug!("Decrypting MLS message");

        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let message = MlsMessage::from_bytes(encrypted_message)?;

        match group.process_incoming_message(message).await? {
            ReceivedMessage::ApplicationMessage(app_msg) => Ok(app_msg.data().to_vec()),
            _ => Err(MlsError::verification_failed(
                "Message was not an application message",
            )),
        }
    }

    pub async fn write_to_storage(&mut self) -> Result<(), MlsError> {
        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;
        group.write_to_storage().await?;
        Ok(())
    }

    pub async fn create_rotation_proposal(&mut self) -> Result<ProposalMsg, MlsError> {
        let crypto_provider = WebCryptoProvider::default();
        let cipher_suite_provider = crypto_provider
            .cipher_suite_provider(CIPHERSUITE)
            .ok_or(MlsError::CiphersuiteUnavailable)?;

        let (secret_key, public_key) = cipher_suite_provider
            .signature_key_generate()
            .await
            .map_err(MlsError::crypto_provider)?;

        self.identity_provider.set_signature_keys(
            secret_key.as_ref().to_vec(),
            public_key.as_ref().to_vec(),
        )?;

        let (new_private_key, new_signing_identity) = self.create_signing_identity(true)?;

        let group = self.group.as_mut().ok_or(MlsError::GroupNotExists)?;

        let update_proposal = group
            .propose_update_with_identity(new_private_key.clone(), new_signing_identity, vec![])
            .await?;

        debug!(
            "Created credential rotation proposal, stored new keys and incremented credential version"
        );

        let ret = update_proposal.to_bytes()?;
        Ok(ret)
    }
}

#[cfg(all(test, not(mls_build_async)))]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use mls_rs_core::identity::MemberValidationContext;

    use crate::errors::MlsError;
    use slim_auth::identity_claims::IdentityClaims;

    use super::*;
    use slim_auth::shared_secret::SharedSecret;

    const SHARED_SECRET: &str = "kjandjansdiasb8udaijdniasdaindasndasndasndasndasndasndasndas";

    #[test]
    fn test_mls_creation() -> Result<(), Box<dyn std::error::Error>> {
        let mut mls = Mls::new(
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
        );

        mls.initialize()?;
        assert!(mls.client.is_some());
        assert!(mls.group.is_none());
        Ok(())
    }

    #[test]
    fn test_group_creation() -> Result<(), Box<dyn std::error::Error>> {
        let mut mls = Mls::new(
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
        );

        mls.initialize()?;
        let _group_id = mls.create_group()?;
        assert!(mls.client.is_some());
        assert!(mls.group.is_some());
        Ok(())
    }

    #[test]
    fn test_key_package_generation() -> Result<(), Box<dyn std::error::Error>> {
        let mut mls = Mls::new(
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
        );

        mls.initialize()?;
        let key_package = mls.generate_key_package()?;
        assert!(!key_package.is_empty());
        Ok(())
    }

    #[test]
    fn test_messaging() -> Result<(), Box<dyn std::error::Error>> {
        let mut alice = Mls::new(
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
        );
        let mut bob = Mls::new(
            SharedSecret::new("bob", SHARED_SECRET).unwrap(),
            SharedSecret::new("bob", SHARED_SECRET).unwrap(),
        );

        alice.initialize()?;
        bob.initialize()?;

        let _group_id = alice.create_group()?;

        let bob_key_package = bob.generate_key_package()?;
        let result = alice.add_member(&bob_key_package)?;
        let welcome = result.welcome_message;
        let _bob_group_id = bob.process_welcome(&welcome)?;

        let message = b"Hello from Alice!";
        let encrypted = alice.encrypt_message(message)?;
        let decrypted = bob.decrypt_message(&encrypted)?;
        assert_eq!(decrypted, message);

        Ok(())
    }

    #[test]
    fn test_credential_rotation() -> Result<(), Box<dyn std::error::Error>> {
        let mut alice = Mls::new(
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
            SharedSecret::new("alice", SHARED_SECRET).unwrap(),
        );
        let mut bob = Mls::new(
            SharedSecret::new("bob", SHARED_SECRET).unwrap(),
            SharedSecret::new("bob", SHARED_SECRET).unwrap(),
        );

        alice.initialize()?;
        bob.initialize()?;

        let _group_id = alice.create_group()?;

        let bob_key_package = bob.generate_key_package()?;
        let result = alice.add_member(&bob_key_package)?;
        let welcome = result.welcome_message;
        let _bob_group_id = bob.process_welcome(&welcome)?;

        let message1 = b"Message before rotation";
        let encrypted1 = alice.encrypt_message(message1)?;
        let decrypted1 = bob.decrypt_message(&encrypted1)?;
        assert_eq!(decrypted1, message1);

        let rotation_proposal = alice.create_rotation_proposal()?;

        let commit = bob.process_proposal(&rotation_proposal, true)?;
        alice.process_commit(&commit)?;

        let message2 = b"Message after rotation from alice";
        let encrypted2 = alice.encrypt_message(message2)?;
        let decrypted2 = bob.decrypt_message(&encrypted2)?;
        assert_eq!(decrypted2, message2);

        assert_eq!(
            alice.get_epoch(),
            bob.get_epoch(),
            "Alice and Bob epochs should match after rotation"
        );

        Ok(())
    }

    fn init_identity(
        name: &str,
        _path: &str,
    ) -> Result<Mls<SharedSecret, SharedSecret>, Box<dyn std::error::Error>> {
        let mut mls = Mls::new(
            SharedSecret::new(name, SHARED_SECRET).unwrap(),
            SharedSecret::new(name, SHARED_SECRET).unwrap(),
        );
        mls.initialize()?;
        Ok(mls)
    }

    fn extract_token_and_pubkey(mls: &Mls<SharedSecret, SharedSecret>) -> (&String, Vec<u8>) {
        let stored = mls
            .stored_identity
            .as_ref()
            .expect("stored identity exists");
        let token = stored
            .last_credential
            .as_ref()
            .expect("stored credential exists");
        (token, stored.public_key_bytes.clone())
    }

    fn build_fake_identity_with_other_key(
        stolen_token: &str,
    ) -> (SigningIdentity, SignaturePublicKey) {
        let (_priv, attacker_pub) =
            Mls::<SharedSecret, SharedSecret>::generate_key_pair().expect("key gen");
        let stolen_cred = BasicCredential::new(stolen_token.as_bytes().to_vec());
        let signing_id = SigningIdentity::new(stolen_cred.into_credential(), attacker_pub.clone());
        (signing_id, attacker_pub)
    }

    fn verify_token_embeds_pubkey(token: &str, expected_pubkey_bytes: &[u8]) {
        let verifier = SharedSecret::new("alice", SHARED_SECRET).unwrap();
        let claims_json: serde_json::Value = verifier.try_get_claims(token).expect("claims");
        let claims = IdentityClaims::from_json(&claims_json).expect("identity claims");
        assert_eq!(
            claims.public_key,
            BASE64.encode(expected_pubkey_bytes),
            "Token must embed expected public key"
        );
    }

    #[test]
    fn test_security_identity_theft_attack() -> Result<(), Box<dyn std::error::Error>> {
        let mut alice = init_identity("alice", "/tmp/mls_test_security_alice")?;
        let mut charlie = init_identity("charlie", "/tmp/mls_test_security_charlie")?;

        let _group_id = alice.create_group()?;
        let charlie_key_package = charlie.generate_key_package()?;
        let charlie_add_res = alice.add_member(&charlie_key_package)?;
        charlie.process_welcome(&charlie_add_res.welcome_message)?;

        let msg = b"Hello from the real Alice!";
        let encrypted = alice.encrypt_message(msg)?;
        let decrypted = charlie.decrypt_message(&encrypted)?;
        assert_eq!(decrypted, msg);

        let (alice_token, alice_pub_bytes) = extract_token_and_pubkey(&alice);

        let (fake_identity, attacker_pub) = build_fake_identity_with_other_key(alice_token);

        let alice_pub_b64 = BASE64.encode(&alice_pub_bytes);
        let attacker_pub_b64 = BASE64.encode(attacker_pub.as_ref());
        assert_ne!(
            alice_pub_b64, attacker_pub_b64,
            "Precondition: attacker key must differ"
        );

        let verifier = SharedSecret::new("alice", SHARED_SECRET).unwrap();
        let provider = SlimIdentityProvider::new(verifier.clone());
        let validation_res =
            provider.validate_member(&fake_identity, None, MemberValidationContext::None);

        assert!(
            matches!(validation_res, Err(MlsError::PublicKeyMismatch { .. })),
            "Expected PublicKeyMismatch for stolen token + different key"
        );

        let claims_json: serde_json::Value = verifier
            .try_get_claims(alice_token.as_str())
            .expect("claims parse");
        let claims = IdentityClaims::from_json(&claims_json).expect("claims map");
        assert_ne!(
            claims.public_key, attacker_pub_b64,
            "Token-bound key must differ from attacker key"
        );

        Ok(())
    }
}
