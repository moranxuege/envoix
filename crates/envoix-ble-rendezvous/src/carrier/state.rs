//! Carrier state machine for SAS verification flow.
//!
//! Tracks the high-level state of the BLE rendezvous from the perspective
//! of the UI layer. The UI calls these methods to drive the handshake and
//! display the SAS code, then confirm or reject it.

use crate::security::authenticator::{
    AuthenticatedParams, EphemeralPublicKey, InitiatorAwaitingSas, InitiatorConfirming,
    InitiatorPending, ResponderAwaitingSas, ResponderConfirming, SasConfirm, SessionKeys,
    initiator_start, responder_respond,
};
use crate::security::sas::{SasCode, verify_sas};
use crate::security::{BleError, mode::BleRendezvousSecurity};

/// High-level carrier verification state, designed to be driven by the
/// platform UI layer (Swift/Kotlin) via FFI or direct method calls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerificationState {
    /// Awaiting ephemeral key exchange with the peer.
    KeyExchange,
    /// SAS code ready for user comparison. The UI must display this code.
    AwaitingUserConfirmation(SasCode),
    /// User confirmed match; confirming with peer.
    Confirming,
    /// Authentication succeeded; session keys derived.
    Verified(SessionKeys),
    /// Authentication failed with an error.
    Failed(String),
}

/// Carrier-side handler that drives the initiator's SAS confirmation UI flow.
pub struct InitiatorCarrier {
    pending: Option<InitiatorPending>,
    awaiting_sas: Option<InitiatorAwaitingSas>,
    confirming: Option<InitiatorConfirming>,
    initiator_presence_id: [u8; 32],
    params: AuthenticatedParams,
    state: VerificationState,
}

impl InitiatorCarrier {
    /// Create a new initiator carrier and start the key exchange.
    pub fn new(
        presence_id: [u8; 32],
        params: AuthenticatedParams,
    ) -> Result<(Self, EphemeralPublicKey), BleError> {
        let (pending, pub_key) = initiator_start(presence_id, params.clone())?;
        Ok((
            Self {
                pending: Some(pending),
                awaiting_sas: None,
                confirming: None,
                initiator_presence_id: presence_id,
                params,
                state: VerificationState::KeyExchange,
            },
            pub_key,
        ))
    }

    /// Current verification state for UI display.
    pub fn state(&self) -> &VerificationState {
        &self.state
    }

    /// Handle receipt of the responder's ephemeral public key.
    /// Transitions to `AwaitingUserConfirmation` with the SAS code.
    pub fn on_responder_key(
        &mut self,
        responder_pub: &[u8; 32],
        responder_presence_id: [u8; 32],
    ) -> Result<&SasCode, BleError> {
        let pending = self.pending.take().ok_or(BleError::Protocol(
            "no pending initiator state".into(),
        ))?;
        let (awaiting_sas, sas) =
            pending.receive_responder_key(responder_pub, responder_presence_id)?;
        self.awaiting_sas = Some(awaiting_sas);
        self.state = VerificationState::AwaitingUserConfirmation(sas);
        match &self.state {
            VerificationState::AwaitingUserConfirmation(sas) => Ok(sas),
            _ => unreachable!(),
        }
    }

    /// User has confirmed that the SAS codes match.
    /// Transitions to `Confirming` and returns the confirmation MAC to send.
    pub fn user_confirmed(&mut self) -> Result<SasConfirm, BleError> {
        let awaiting = self.awaiting_sas.take().ok_or(BleError::Protocol(
            "no SAS awaiting state".into(),
        ))?;
        let (confirming, confirm) = awaiting.confirm()?;
        self.confirming = Some(confirming);
        self.state = VerificationState::Confirming;
        Ok(confirm)
    }

    /// User has rejected the SAS codes (they didn't match).
    pub fn user_rejected(&mut self) {
        self.state = VerificationState::Failed("user rejected SAS mismatch".into());
        self.awaiting_sas = None;
        self.pending = None;
        self.confirming = None;
    }

    /// Handle receipt of the responder's confirmation MAC.
    pub fn on_responder_confirm(&mut self, responder_confirm: &SasConfirm) -> Result<(), BleError> {
        let confirming = self.confirming.take().ok_or(BleError::Protocol(
            "no confirming state".into(),
        ))?;
        let keys = confirming.verify_responder(responder_confirm)?;
        self.state = VerificationState::Verified(keys);
        Ok(())
    }
}

/// Carrier-side handler that drives the responder's SAS confirmation UI flow.
pub struct ResponderCarrier {
    awaiting_sas: Option<ResponderAwaitingSas>,
    confirming: Option<ResponderConfirming>,
    state: VerificationState,
}

impl ResponderCarrier {
    /// Create a new responder carrier given the initiator's ephemeral key.
    pub fn new(
        presence_id: [u8; 32],
        params: AuthenticatedParams,
        initiator_pub: &[u8; 32],
        initiator_presence_id: [u8; 32],
    ) -> Result<(Self, EphemeralPublicKey, SasCode), BleError> {
        let (awaiting_sas, pub_key, sas) =
            responder_respond(presence_id, params, initiator_pub, initiator_presence_id)?;
        Ok((
            Self {
                awaiting_sas: Some(awaiting_sas),
                confirming: None,
                state: VerificationState::AwaitingUserConfirmation(sas),
            },
            pub_key,
            sas,
        ))
    }

    /// Current verification state for UI display.
    pub fn state(&self) -> &VerificationState {
        &self.state
    }

    /// User has confirmed that the SAS codes match.
    pub fn user_confirmed(&mut self) -> Result<SasConfirm, BleError> {
        let awaiting = self.awaiting_sas.take().ok_or(BleError::Protocol(
            "no SAS awaiting state".into(),
        ))?;
        let (confirming, confirm) = awaiting.confirm()?;
        self.confirming = Some(confirming);
        self.state = VerificationState::Confirming;
        Ok(confirm)
    }

    /// User has rejected the SAS codes.
    pub fn user_rejected(&mut self) {
        self.state = VerificationState::Failed("user rejected SAS mismatch".into());
        self.awaiting_sas = None;
        self.confirming = None;
    }

    /// Handle receipt of the initiator's confirmation MAC.
    pub fn on_initiator_confirm(&mut self, initiator_confirm: &SasConfirm) -> Result<(), BleError> {
        let confirming = self.confirming.take().ok_or(BleError::Protocol(
            "no confirming state".into(),
        ))?;
        let keys = confirming.verify_initiator(initiator_confirm)?;
        self.state = VerificationState::Verified(keys);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_params() -> AuthenticatedParams {
        AuthenticatedParams {
            security_mode: BleRendezvousSecurity::AuthenticatedV1,
            initiator_identity: None,
            responder_identity: None,
            invite_mode: 1,
            directional_role: 0,
            exchange_id: None,
            sender_ticket: None,
            receiver_ticket: None,
            broker_context: None,
            expiry: 0,
            invitation_digest: [0xBBu8; 32],
            fragment_max_size: 512,
            fragment_timeout_ms: 30_000,
        }
    }

    #[test]
    fn initiator_full_flow() {
        let i_pid = [0x11u8; 32];
        let r_pid = [0x22u8; 32];
        let params = test_params();

        let (mut initiator, i_pub) = InitiatorCarrier::new(i_pid, params.clone()).unwrap();
        assert_eq!(initiator.state(), &VerificationState::KeyExchange);

        // Simulate responder generating its key
        let (mut responder, r_pub, r_sas) =
            ResponderCarrier::new(r_pid, params, &i_pub.key, i_pid).unwrap();

        // Initiator receives responder's key
        let i_sas = initiator
            .on_responder_key(&r_pub.key, r_pid)
            .unwrap()
            .clone();
        assert_eq!(i_sas, r_sas);
        assert!(matches!(
            initiator.state(),
            VerificationState::AwaitingUserConfirmation(_)
        ));

        // Both users confirm
        let i_confirm = initiator.user_confirmed().unwrap();
        let r_confirm = responder.user_confirmed().unwrap();

        // Exchange confirmations
        initiator.on_responder_confirm(&r_confirm).unwrap();
        responder.on_initiator_confirm(&i_confirm).unwrap();

        assert!(matches!(initiator.state(), VerificationState::Verified(_)));
        assert!(matches!(responder.state(), VerificationState::Verified(_)));
    }

    #[test]
    fn user_rejection_aborts() {
        let i_pid = [0x11u8; 32];
        let r_pid = [0x22u8; 32];
        let params = test_params();

        let (mut initiator, i_pub) = InitiatorCarrier::new(i_pid, params.clone()).unwrap();
        let (_responder, r_pub, _r_sas) =
            ResponderCarrier::new(r_pid, params, &i_pub.key, i_pid).unwrap();
        let _i_sas = initiator.on_responder_key(&r_pub.key, r_pid).unwrap();

        // User rejects
        initiator.user_rejected();
        assert!(matches!(initiator.state(), VerificationState::Failed(_)));
    }
}
