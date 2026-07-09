## Title

Support Sender-Initiated Transfer Flows

## Problem

Envoix can transfer files today, but the setup flow is backwards for normal use.

In most flows, the receiver must act first:

1. The receiver opens Receive.
2. The receiver creates a room, link, or token flow.
3. The sender enters or scans that invite.
4. The sender starts the transfer.

This makes the product receiver-first. Most users expect the opposite:

```text
select the file -> choose or invite the receiving device -> send
```

The current technical blocker is that the protocol still assumes:

```text
the peer who opens the connection is also the peer who sends file bytes
```

Because of that, Envoix cannot naturally support this flow yet:

```text
sender selects file -> sender creates QR/link -> receiver scans -> transfer starts
```

## Current Behavior

The new client API already has a `PeerSource` abstraction, but the combinations needed for sender-initiated setup are still unsupported:

- `send(..., PeerSource::ShowInvite)`
- `send(..., PeerSource::ShowManual)`
- `receive(..., PeerSource::Invite)`
- `receive(..., PeerSource::Manual)`

Room mode has the same limitation. The receiver submits a real data endpoint, but the sender still submits placeholder endpoint data because the current implementation assumes the sender only dials.

## Goal

Allow the sender to start the user flow.

After this issue, both flows should be supported:

### Existing Flow

```text
receiver creates invite -> sender joins -> sender sends file
```

### New Flow

```text
sender selects file -> sender creates invite -> receiver joins -> sender sends file
```

In other words, the file sender must not always have to be the connection initiator.

## Required Changes

### 1. Add Explicit Transfer Role Negotiation

After a connection is established, peers must explicitly declare who sends file data and who receives it.

Do not infer file direction from dial direction.

Conceptually:

```text
Hello {
  protocol_version,
  data_role: sender | receiver
}
```

Role mismatches should fail with a clear protocol error.

### 2. Update SPAKE2 Role Binding

The current SPAKE2 authentication flow assumes sender/receiver roles.

Update the auth transcript so it cannot confuse:

```text
connection initiator / acceptor
```

with:

```text
file sender / receiver
```

The final design may bind authentication to connection roles, or include both connection role and file role, but role confusion must be impossible.

### 3. Support Sender-Produced Invites

Implement:

```text
send(file, PeerSource::ShowInvite)
```

Expected behavior:

1. The sender selects a file.
2. The sender listens and emits an invite.
3. The receiver scans or pastes the invite.
4. The receiver connects to the sender.
5. After role negotiation, the sender sends the file.

### 4. Support Receiver Joining an Invite

Implement:

```text
receive(output_dir, PeerSource::Invite)
```

Expected behavior:

1. The receiver parses the invite.
2. The receiver connects to the invite producer.
3. The receiver declares itself as the data receiver.
4. The receiver receives the file.

### 5. Add an Offer / Accept Step

Before file bytes are sent, the sender should offer basic file metadata:

```text
Offer {
  file_name,
  file_size
}
```

The receiver responds:

```text
Accept | Reject
```

The CLI may auto-accept to preserve current one-shot behavior. Native clients can later show a confirmation prompt.

### 6. Stop Using Placeholder Endpoint Data in Room Mode

During room pairing, both peers should submit real endpoint data where possible.

The existing room flow must continue to work, but it should no longer rely on the sender uploading fake placeholder endpoint data.

## Out of Scope

- Trusted devices
- Persistent device lists
- Full Activity redesign
- Multi-file or folder transfer
- Parallel transfer
- Speed limiting
- Reverse-dial fallback
- Production-grade end-to-end file encryption

Reverse-dial fallback can be added in a follow-up issue once both peers exchange real endpoint data.

## Acceptance Criteria

- Existing receiver-first invite flow still works.
- Existing room flow still works.
- Existing mDNS token flow still works.
- A new integration test covers `send(..., PeerSource::ShowInvite)`.
- A new integration test covers `receive(..., PeerSource::Invite)`.
- A test covers the case where the connection initiator is not the file sender.
- Role mismatch is rejected with a clear protocol error.
- Room sender no longer needs placeholder endpoint data.
- Documentation explains that these are separate concepts:
  - rendezvous method: how peers find each other;
  - connection direction: who opens the connection;
  - file direction: who sends bytes.
