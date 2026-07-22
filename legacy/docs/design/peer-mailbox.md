# The peer mailbox (design)

Formalization of the async channel first built for completion receipts
(`architecture-review-2026-07.md` §4b, shipped e603058), now that it has a
second user (cancel tombstones, `transfer-state-machine.md` D1) and a clear
shape. Status: DESIGN, receipts already conform (modulo key namespacing below).

## Concept

A paired transfer has two communication channels:

| | QUIC connection | **Peer mailbox** |
|---|---|---|
| when | both peers online, path alive | any time within the TTL |
| delivery | one-shot (dies with the connection) | **idempotent, retryable for hours** |
| carries | the data plane + control frames | small, sealed, per-transfer notices |
| role | the transfer itself | the *time-shifted* half of the peer link |

The mailbox exists because some messages matter most at exactly the moment the
connection is least reliable (the CompleteAck dies on a dying path; a cancel
notice is lost precisely when the peer is unreachable). Store-and-forward via
the rdz — the one party that is always online — moves those messages into a
different TIME domain, which is the property the one-shot channel cannot have.

**The mailbox is an accelerator, never a dependency**: every flow it serves
must also work (slower, with peer presence) through the connection-based path.

## Addressing

```
slot_key = hex(blake3(transfer_id ‖ "\n" ‖ kind))
```

- One slot per (transfer, kind). Writes are last-write-wins on the slot.
- `transfer_id` is high-entropy random and shared only over the authenticated
  channel — possession gates retrieval; the server sees opaque keys.
- `kind` namespacing keeps message types in separate slots (a receipt can
  never shadow a tombstone).

## Envelope

```
blob = seal_json( key = KDF(transfer_id ‖ "\n" ‖ code),   // blake3 derive_key
                  aad = "envoix-mailbox-v1:" ‖ kind,
                  payload )
```

- ChaCha20Poly1305 via the `envoix-pairing` bundle primitives.
- The AAD binds the blob to the scheme version AND the kind — a sealed receipt
  cannot be replayed into the tombstone slot and open successfully.
- The KDF mixes the transfer id (entropy) with the pairing code (the shared
  secret); the rdz operator can neither read nor brute-force blobs.

## Semantics (the rules every kind follows)

1. **Write**: idempotent PUT; retry with backoff for as long as the fact holds
   (a receipt stays true forever; re-post freely).
2. **Read**: idempotent GET. **Never delete-on-read** — a read-once slot would
   reintroduce exactly the one-shot fragility the mailbox exists to cure (a
   lost GET response would destroy the only proof). Expiry is by TTL only
   (server: 7 days, in-memory; a server restart drops blobs, which is safe
   because of rule 4).
3. **Trust**: a mailbox message is a *hint with proof*. The state machine acts
   only on blobs that (a) open under the AEAD and (b) pass kind-specific
   content checks (receipts: size + BLAKE3 equality against the local file).
   Anything else is ignored silently.
4. **Containment**: mailbox unreachable / empty / expired ⇒ the peer-present
   path still resolves the situation. No flow may *require* the mailbox.
5. **Polling is state-scoped and bounded**: each machine state with an open
   out-of-band question defines its own finite poll schedule; no background
   daemons. Re-polling resumes only on a user action (Retry/Resume) or state
   re-entry.

## Kind registry (v1)

| kind | direction | payload | written when | read when (machine state) | effect on verify |
|---|---|---|---|---|---|
| `receipt` | receiver → sender | `TransferReceipt {name, size, blake3}` | finalize (post w/ backoff; re-post on later retries) | `Unconfirmed` (2s/10s/30s, + on Retry) | → `Completed` ("confirmed via receipt") |
| `cancel` *(v2)* | either → peer | `{cancelled_at_bytes}` (content TBD) | local Cancel when the in-band notice may not have been delivered | `Paused(Lost)` (once on entry, + on failed Resume) | → `Cancelled`, discard partial (D1) |

Future kinds reserve the same envelope; adding one is a registry row, a poll
schedule, and a machine edge — no new infrastructure.

## Server (unchanged by this formalization)

`POST/GET /receipts/{key}` — the path name predates the generalization; the
server is deliberately kind-agnostic (opaque keys, opaque bytes, 4 KB cap,
TTL, entry cap). Renaming to `/mailbox/{key}` can ride any future server
change; it is cosmetic.

## Migration note

Receipts shipped (e603058) with `slot_key = blake3(transfer_id)` and
`aad = "envoix-receipt-v1"`. Conforming them to the namespaced scheme is a
client-only change (the server never interprets keys); the fleet ships both
sides together, and pending blobs age out within the TTL. Done alongside this
doc.
