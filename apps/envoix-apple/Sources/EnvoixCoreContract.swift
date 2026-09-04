import EnvoixCore

let expectedCoreFFIAPIVersion: UInt32 = 26
let expectedApplicationBindingVersion: UInt32 = 1
let expectedApplicationContractVersion: UInt16 = 6
let expectedAgentProtocolVersion: UInt16 = 13

let expectedRoomControlCoreCapability = "foreground_room_control_v5"
let expectedNearbyInviteCoreCapability = "nearby_invite_inbox_v1"
let expectedFailureProjectionCoreCapability = "canonical_failure_projection_v1"
let expectedRoomControlErrorCoreCapability = "typed_room_control_errors_v1"
let expectedRememberedCredentialVaultCapability = "typed_remembered_credential_vault_v1"
let expectedTypedApplicationCapability = "typed_application_contract_v6"
let expectedPersistentApplicationEngineCapability = "persistent_application_engine_v1"
let expectedAgentHostControlCapability = "agent_host_control_v3"
let expectedDeploymentEndpointsCapability = "deployment_endpoints_v1"

func coreMatchesExpectedRoomControlContract(_ info: FfiCoreInfo) -> Bool {
    info.ffiApiVersion == expectedCoreFFIAPIVersion
        && info.capabilities.contains(expectedRoomControlCoreCapability)
        && info.capabilities.contains(expectedNearbyInviteCoreCapability)
        && info.capabilities.contains(expectedFailureProjectionCoreCapability)
        && info.capabilities.contains(expectedRoomControlErrorCoreCapability)
        && info.capabilities.contains(expectedRememberedCredentialVaultCapability)
        && info.capabilities.contains(expectedTypedApplicationCapability)
        && info.capabilities.contains(expectedPersistentApplicationEngineCapability)
        && info.capabilities.contains(expectedAgentHostControlCapability)
        && info.capabilities.contains(expectedDeploymentEndpointsCapability)
}
