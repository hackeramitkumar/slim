// Copyright AGNTCY Contributors (https://github.com/agntcy)
// SPDX-License-Identifier: Apache-2.0

#![cfg(target_arch = "wasm32")]

use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use slim_wasm::GroupChat;

const SHARED_SECRET: &str = "kjandjansdiasb8udaijdniasdaindasndasndasndasndasndasndasndas";

#[wasm_bindgen_test]
async fn test_group_chat_init() {
    let gc = GroupChat::new("alice", SHARED_SECRET).await;
    assert!(gc.is_ok(), "GroupChat::new should succeed");

    let gc = gc.unwrap();
    assert!(gc.group_id().is_none(), "no group created yet");
    assert!(gc.epoch().is_none(), "no epoch before group creation");
}

#[wasm_bindgen_test]
async fn test_create_group() {
    let mut gc = GroupChat::new("alice", SHARED_SECRET).await.unwrap();
    let group_id = gc.create_group().await;
    assert!(group_id.is_ok(), "create_group should succeed");

    let gid = group_id.unwrap();
    assert!(gid.length() > 0, "group id should be non-empty");
    assert!(gc.group_id().is_some(), "group_id getter should return Some");
    assert!(gc.epoch().is_some(), "epoch should be Some after group creation");
}

#[wasm_bindgen_test]
async fn test_key_package_generation() {
    let gc = GroupChat::new("alice", SHARED_SECRET).await.unwrap();
    let kp = gc.generate_key_package().await;
    assert!(kp.is_ok(), "key package generation should succeed");

    let kp_bytes = kp.unwrap();
    assert!(kp_bytes.length() > 0, "key package should be non-empty");
}

#[wasm_bindgen_test]
async fn test_add_member_and_join() {
    let mut alice = GroupChat::new("alice", SHARED_SECRET).await.unwrap();
    let mut bob = GroupChat::new("bob", SHARED_SECRET).await.unwrap();

    alice.create_group().await.unwrap();

    let bob_kp = bob.generate_key_package().await.unwrap();
    let result = alice.add_member(&bob_kp.to_vec()).await;
    assert!(result.is_ok(), "add_member should succeed: {:?}", result.err());

    let result_obj = result.unwrap();
    let welcome = js_sys::Reflect::get(&result_obj, &JsValue::from_str("welcomeMessage"))
        .unwrap();
    let welcome_bytes = js_sys::Uint8Array::from(welcome);
    assert!(welcome_bytes.length() > 0, "welcome message should be non-empty");

    let joined = bob.join_group(&welcome_bytes.to_vec()).await;
    assert!(joined.is_ok(), "join_group should succeed: {:?}", joined.err());
    assert!(bob.group_id().is_some(), "bob should have a group after joining");
}

#[wasm_bindgen_test]
async fn test_encrypt_decrypt() {
    let mut alice = GroupChat::new("alice", SHARED_SECRET).await.unwrap();
    let mut bob = GroupChat::new("bob", SHARED_SECRET).await.unwrap();

    alice.create_group().await.unwrap();

    let bob_kp = bob.generate_key_package().await.unwrap();
    let result = alice.add_member(&bob_kp.to_vec()).await.unwrap();
    let welcome = js_sys::Reflect::get(&result, &JsValue::from_str("welcomeMessage")).unwrap();
    let welcome_bytes = js_sys::Uint8Array::from(welcome);
    bob.join_group(&welcome_bytes.to_vec()).await.unwrap();

    let plaintext = b"Hello from Alice in the browser!";
    let encrypted = alice.encrypt(plaintext).await;
    assert!(encrypted.is_ok(), "encrypt should succeed: {:?}", encrypted.err());

    let ciphertext = encrypted.unwrap();
    assert!(ciphertext.length() > 0, "ciphertext should be non-empty");
    assert_ne!(
        ciphertext.to_vec(),
        plaintext.to_vec(),
        "ciphertext must differ from plaintext"
    );

    let decrypted = bob.decrypt(&ciphertext.to_vec()).await;
    assert!(decrypted.is_ok(), "decrypt should succeed: {:?}", decrypted.err());
    assert_eq!(
        decrypted.unwrap().to_vec(),
        plaintext.to_vec(),
        "decrypted message must match original"
    );
}

#[wasm_bindgen_test]
async fn test_bidirectional_messaging() {
    let mut alice = GroupChat::new("alice", SHARED_SECRET).await.unwrap();
    let mut bob = GroupChat::new("bob", SHARED_SECRET).await.unwrap();

    alice.create_group().await.unwrap();

    let bob_kp = bob.generate_key_package().await.unwrap();
    let result = alice.add_member(&bob_kp.to_vec()).await.unwrap();
    let welcome = js_sys::Reflect::get(&result, &JsValue::from_str("welcomeMessage")).unwrap();
    bob.join_group(&js_sys::Uint8Array::from(welcome).to_vec()).await.unwrap();

    let msg1 = b"Hello Bob!";
    let enc1 = alice.encrypt(msg1).await.unwrap();
    let dec1 = bob.decrypt(&enc1.to_vec()).await.unwrap();
    assert_eq!(dec1.to_vec(), msg1.to_vec());

    let msg2 = b"Hello Alice!";
    let enc2 = bob.encrypt(msg2).await.unwrap();
    let dec2 = alice.decrypt(&enc2.to_vec()).await.unwrap();
    assert_eq!(dec2.to_vec(), msg2.to_vec());
}

#[wasm_bindgen_test]
async fn test_epoch_advances() {
    let mut alice = GroupChat::new("alice", SHARED_SECRET).await.unwrap();
    let mut bob = GroupChat::new("bob", SHARED_SECRET).await.unwrap();

    alice.create_group().await.unwrap();
    let epoch_before = alice.epoch().unwrap();

    let bob_kp = bob.generate_key_package().await.unwrap();
    let result = alice.add_member(&bob_kp.to_vec()).await.unwrap();
    let welcome = js_sys::Reflect::get(&result, &JsValue::from_str("welcomeMessage")).unwrap();
    bob.join_group(&js_sys::Uint8Array::from(welcome).to_vec()).await.unwrap();

    let epoch_after = alice.epoch().unwrap();
    assert!(
        epoch_after > epoch_before,
        "epoch should advance after adding a member"
    );

    assert_eq!(
        alice.epoch(),
        bob.epoch(),
        "alice and bob should be on the same epoch"
    );
}
