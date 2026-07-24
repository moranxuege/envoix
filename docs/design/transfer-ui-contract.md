# Transfer UI contract

Status: frozen for the Manifest v2 application integration.

Apple and Android use platform-native views, but they present the same transfer
lifecycle and permit the same user actions. Native UI code may format,
localize, and lay out this model differently; it must not invent another state
machine.

## States

| State | Meaning |
| --- | --- |
| `preparing` | Local source preparation before a session starts |
| `waiting_for_peer` | A local invitation is ready and no peer has joined |
| `pairing` | The peers are authenticating the invitation |
| `connecting` | An authenticated transfer path is being established |
| `awaiting_decision` | The receiver must accept or choose a destination |
| `transferring` | Payload bytes are moving; direction determines Send/Receive wording |
| `verifying` | Payload transfer is complete and content is being verified |
| `saving` | The receiver is publishing verified content |
| `waiting_for_receiver_save` | The sender is waiting for receiver publication |
| `finalizing_delivery` | The receiver saved successfully and delivery proof is finishing |
| `paused` | The current attempt stopped while resumable state is retained |
| `delivered` | Receiver publication and delivery proof both completed |
| `failed` | The attempt failed with a structured cause |
| `canceled` | The user abandoned the transfer |

`receiving` is presentation wording for a receive-direction transfer; it is not
a separate lifecycle state. `idle` belongs to the setup screen, not to a
transfer Activity.

## Actions

| State | Actions |
| --- | --- |
| `preparing` | Cancel |
| `waiting_for_peer`, `pairing`, `connecting`, `transferring`, `verifying` | Pause, Cancel |
| `awaiting_decision` | Accept, Cancel |
| `saving`, `waiting_for_receiver_save`, `finalizing_delivery` | None |
| `paused` | Resume, Cancel |
| `failed` | Recovery action only when the structured failure permits it; Remove |
| `canceled` | Remove |
| `delivered` | Open/Share when a saved result exists; Remove |

The service/view model must enforce the same policy as the visible controls.
Hiding an invalid button is not sufficient.

## Progress continuity

- Bytes never decrease within one attempt.
- A new resume attempt may establish a new baseline, but stale callbacks from
  an older attempt are ignored.
- Rate and ETA appear only while payload bytes are moving.
- Verification, saving, receiver-save wait, and delivery finalization keep a
  full progress bar visible so that 100% payload does not look like a stalled
  transfer.
- Paused and failed cards retain the last confirmed byte count.
- Within a live presentation session, `delivered`, `failed`, and `canceled`
  are sticky until an explicit new attempt or removal. Cross-process retry
  scheduling and terminal-history retention remain Issue #56 policy.

## Presentation

Both platforms use one stable transfer card:

1. item or inventory title and Send/Receive direction;
2. lifecycle stage and optional path;
3. one progress location;
4. byte/rate/ETA metrics appropriate to the stage;
5. actions derived only from the policy above;
6. expandable diagnostics and authenticated inventory.

Completion wording is `Delivered`, not merely `Transferred`, because sender
completion is gated by receiver publication and verified delivery proof.
