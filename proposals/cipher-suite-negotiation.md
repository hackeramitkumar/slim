# Proposal: Cross-Platform Cipher Suite Negotiation & Migration for MLS Groups

**Author:** @amitami2
**Status:** Draft
**Created:** 2026-04-18

---

## Summary

SLIM uses MLS (RFC 9420) for end-to-end encryption in group sessions. Today, all participants in a group must share a single cipher suite. This creates a hard interoperability wall between native clients (which default to `CURVE25519_AES128`) and browser/WASM clients (which are limited to `P256_AES128` by WebCrypto). A browser participant simply cannot join a Curve25519 group, and vice versa.

This proposal introduces **cipher suite negotiation** during participant discovery and **live cipher suite migration** for existing groups, enabling true cross-platform group communication without manual reconfiguration.

---

## Motivation

### The Problem

| Platform | Crypto backend | Default cipher suite | All supported suites |
|----------|---------------|---------------------|---------------------|
| Native (Linux, macOS, Windows) | aws-lc (`mls-rs-crypto-awslc`) | `CURVE25519_AES128` (0x0001) | `CURVE25519_AES128`, `P256_AES128` |
| Browser / WASM | WebCrypto (`mls-rs-crypto-webcrypto`) | `P256_AES128` (0x0002) | `P256_AES128` only |

WebCrypto does not expose Curve25519 operations, so WASM clients **cannot** participate in a `CURVE25519_AES128` group. Without negotiation, a moderator must know upfront which cipher suite to use, and an existing group has no path to accommodate a participant with different capabilities.

### Goals

1. **Automatic negotiation** -- the moderator selects the best common cipher suite during discovery, with no user intervention.
2. **Live migration** -- an existing group transparently migrates to a new cipher suite when a joining participant requires it.
3. **Backward compatibility** -- older clients that do not advertise cipher suites continue to work with platform defaults.
4. **Minimal protocol overhead** -- reuse existing MLS key-package and welcome flows; add only one new control message type.

### Non-Goals

- Supporting multiple concurrent cipher suites within a single group (MLS does not allow this).
- Negotiating non-MLS parameters (compression, padding, etc.).
- Automatic cipher "upgrade" when a stronger suite becomes available and all members support it (possible future extension).

---

## Design

### 1. Capability Advertisement

Participants advertise their supported cipher suites in the existing `DiscoveryReply`:

```protobuf
message DiscoveryReplyPayload {
  // RFC 9420 cipher suite identifiers supported by this client.
  // Empty list = "platform default only" (backward compat).
  repeated uint32 supported_cipher_suites = 1;
}
```

On the channel/session layer, the list is populated from the platform's MLS build:

- **Native:** `[0x0001 (CURVE25519_AES128), 0x0002 (P256_AES128)]`
- **WASM:** `[0x0002 (P256_AES128)]`

### 2. Negotiation Algorithm

When the moderator receives a `DiscoveryReply` with a non-empty `supported_cipher_suites` list, it runs a simple **moderator-preference-ordered intersection**:

```
negotiate(moderator_suites, participant_suites) -> Option<CipherSuite>
    for suite in moderator_suites:
        if suite in participant_suites:
            return Some(suite)
    return None   // no common suite -- reject participant
```

The moderator's list is ordered by preference (strongest/fastest first). The first match wins. If no common suite exists, the join is rejected with `CipherSuiteNegotiationFailed`.

### 3. Cipher Suite Selection on Join

The negotiated suite is communicated to the joining participant via the existing `JoinRequest`:

```protobuf
message JoinRequestPayload {
  // ...existing fields...
  // Cipher suite selected by the moderator for this group.
  // 0 = platform default (backward compat with old moderators).
  uint32 selected_cipher_suite = 4;
}
```

If `selected_cipher_suite` differs from the participant's current MLS configuration, the participant re-initializes its MLS client with the new suite before generating its key package.

### 4. Live Migration for Existing Groups

This is the core of the proposal. When a participant that does not support the group's current cipher suite attempts to join, the moderator orchestrates a **full group migration** before completing the join.

#### New Control Message

```protobuf
// SESSION_MESSAGE_TYPE_CIPHER_MIGRATION = 19

message CipherMigrationPayload {
  // The new cipher suite ID (RFC 9420 value) that the group will use.
  uint32 new_cipher_suite = 1;
}
```

#### Migration Protocol Flow

```
Moderator                    Existing Members (P1..Pn)         New Participant (Pnew)
    |                                |                               |
    |<------ DiscoveryReply ---------|-------------------------------|
    |   (Pnew supports P256 only;   |                               |
    |    group is CURVE25519)        |                               |
    |                                |                               |
    | --- stash Pnew's discovery --- |                               |
    |                                |                               |
    |--- CipherMigration(P256) ---->|                               |
    |    (same msg_id to all)        |                               |
    |                                |                               |
    |    [P1..Pn destroy MLS group,  |                               |
    |     reinit with P256,          |                               |
    |     generate fresh key pkg]    |                               |
    |                                |                               |
    |<----- JoinReply(key_pkg) -----|                               |
    |    (each member replies        |                               |
    |     with same msg_id)          |                               |
    |                                |                               |
    | [all key packages collected]   |                               |
    |                                |                               |
    | [moderator destroys own group, |                               |
    |  reinits with P256,            |                               |
    |  creates new MLS group,        |                               |
    |  adds each member via KP]      |                               |
    |                                |                               |
    |--- GroupWelcome(welcome) ----->|  (per-member welcome)         |
    |                                |                               |
    | [migration complete]           |                               |
    | [resume stashed discovery]     |                               |
    |                                |                               |
    |--- JoinRequest(P256) -------->|------------------------------>|
    |<------ JoinReply(key_pkg) ----|------------------------------>|
    |--- GroupAdd/Welcome --------->|------------------------------>|
    |                                |                               |
    | [Pnew is now a full member     |                               |
    |  of the P256 group]            |                               |
```

#### State Machine (Moderator)

The migration is modeled as a `ModeratorTask::Migrate(MigrateCipherSuite)` with a two-phase commit:

```
Phase 1: Migration Request
  - Broadcast CipherMigration to all existing members
  - Start timer; collect JoinReply key packages
  - Complete when all expected key packages arrive

Phase 2: Group Recreation & Welcome
  - Moderator destroys old group, creates new group with new suite
  - Adds each member using collected key packages
  - Sends per-member GroupWelcome
  - Complete when all welcomes sent

Post-migration:
  - Clear task; restore stashed DiscoveryReply
  - Re-enter normal join flow for the new participant
```

#### Participant Behavior on `CipherMigration`

1. Destroy existing MLS group state (`destroy_group`).
2. Reinitialize MLS client with the new cipher suite.
3. Generate a fresh key package.
4. Reply with `JoinReply` containing the key package, using the same `message_id` as the migration request.

#### Edge Cases

| Scenario | Behavior |
|----------|----------|
| **No existing members** (moderator-only group) | Moderator locally recreates group with new suite; no broadcast needed; immediately resumes join flow |
| **No common suite** | Join rejected with `CipherSuiteNegotiationFailed`; existing group untouched |
| **Empty `supported_cipher_suites`** | Negotiation skipped entirely; old behavior preserved (platform default assumed) |
| **Member fails to reply** | Migration timer expires; task fails with `ModeratorTaskMigrateFailed`; group state preserved (no partial migration) |
| **Multiple joins requiring different migrations** | Serialized via `current_task`; second join stashed until first migration completes |

---

## Security Considerations

1. **Forward secrecy preserved** -- migration creates an entirely new MLS group with fresh key material. Old group secrets are destroyed. No key material is carried over.

2. **No downgrade without cause** -- migration is triggered only when a joining participant lacks support for the current suite. The moderator's preference ordering ensures the strongest mutually-supported suite is selected.

3. **Moderator trust model unchanged** -- the moderator already controls group membership. Migration extends this authority to cipher suite selection, consistent with MLS's tree-based key management where the committer (moderator) drives group state.

4. **Atomicity** -- if any participant fails to complete migration (timeout, disconnect), the entire migration fails and the original group state is preserved. There is no partial-migration state.

5. **Replay protection** -- migration uses the same message-ID correlation as existing MLS operations. Welcome messages are unique per-member and bound to the new group epoch.

---

## Wire Format Changes

| Change | Type | Proto field | Breaking? |
|--------|------|-------------|-----------|
| `DiscoveryReplyPayload.supported_cipher_suites` | New repeated field | `repeated uint32 ... = 1` | No -- empty list = backward compat |
| `JoinRequestPayload.selected_cipher_suite` | New field | `uint32 ... = 4` | No -- `0` = platform default |
| `CipherMigrationPayload` | New message | `uint32 new_cipher_suite = 1` | No -- new message type `19` |
| `SessionMessageType.CIPHER_MIGRATION` | New enum value | `= 19` | No -- unknown types ignored by old clients |

All changes are additive. Old clients that do not populate `supported_cipher_suites` will be treated as supporting only their platform default, and will not trigger or participate in migration.

---

## Implementation Notes

### Platform Cipher Suite Support

The `supported_cipher_suites()` function is compile-time determined:

```rust
// Native (aws-lc): both suites available
#[cfg(feature = "native")]
pub fn supported_cipher_suites() -> Vec<CipherSuite> {
    vec![CipherSuite::CURVE25519_AES128, CipherSuite::P256_AES128]
}

// WASM (WebCrypto): P256 only
#[cfg(all(feature = "wasm", not(feature = "native")))]
pub fn supported_cipher_suites() -> Vec<CipherSuite> {
    vec![CipherSuite::P256_AES128]
}
```

This means a native moderator can negotiate down to P256 when a browser joins, but a WASM moderator can never host a Curve25519 group.

### MLS Group Lifecycle During Migration

```
[Existing group: CURVE25519]
         |
    destroy_group()          -- drop MLS group state
         |
    reinitialize(P256)       -- reconfigure MLS client
         |
    create_group()           -- new MLS group with P256
         |
    add_member(kp) x N       -- add each member's fresh key package
         |
    send welcome x N         -- per-member welcome message
         |
[New group: P256, all members re-keyed]
```

### Relationship to MLS ReInit

MLS RFC 9420 defines a ReInit proposal (Section 12.1.5) for changing a group's cipher suite. This implementation takes a more direct approach -- destroying and recreating the group -- for several reasons:

1. ReInit requires all members to process the proposal through the MLS tree, which adds complexity with mixed-platform crypto backends.
2. The destroy-and-recreate approach gives the moderator full control over timing and error handling.
3. The result is identical: all members end up in a new group with fresh key material and the new cipher suite.

A future optimization could explore using MLS ReInit for groups where all members support the new suite natively.

---

## Future Extensions

- **Cipher suite upgrade**: periodically check if all members now support a stronger suite and migrate proactively.
- **Participant-initiated negotiation**: allow a participant to request migration (currently only moderator-driven).
- **Multi-suite advertisement in JoinRequest**: allow the moderator to suggest multiple acceptable suites, letting the participant choose.
- **MLS ReInit integration**: use the protocol-native ReInit proposal when the `mls-rs` ecosystem adds cross-platform ReInit support.

---

## References

- [RFC 9420 - The Messaging Layer Security (MLS) Protocol](https://www.rfc-editor.org/rfc/rfc9420)
- [MLS Cipher Suites (RFC 9420, Section 17.1)](https://www.rfc-editor.org/rfc/rfc9420#section-17.1)
- [WebCrypto API - Supported Algorithms](https://www.w3.org/TR/WebCryptoAPI/)
- [mls-rs - Rust MLS implementation](https://github.com/awslabs/mls-rs)
