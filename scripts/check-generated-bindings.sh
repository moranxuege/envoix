#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" != "1" ]]; then
  exec "$repo_root/scripts/with-build-cache-guard.sh" "$0" "$@"
fi

case "$(uname -s)" in
  Darwin) library="$repo_root/target/debug/libenvoix_ffi.dylib" ;;
  Linux) library="$repo_root/target/debug/libenvoix_ffi.so" ;;
  *)
    echo "error: generated binding check supports macOS and Linux hosts" >&2
    exit 2
    ;;
esac

output_root="$(mktemp -d "${TMPDIR:-/tmp}/envoix-bindings.XXXXXX")"
trap 'rm -rf -- "$output_root"' EXIT INT TERM

cd "$repo_root"
cargo build -p envoix-ffi --features bindgen-cli --lib --bin envoix-bindgen

target/debug/envoix-bindgen generate \
  --language kotlin \
  --no-format \
  --out-dir "$output_root/kotlin" \
  --config crates/envoix-ffi/uniffi.toml \
  "$library"
target/debug/envoix-bindgen generate \
  --language swift \
  --no-format \
  --out-dir "$output_root/swift" \
  --config crates/envoix-ffi/uniffi.toml \
  "$library"
python3 scripts/postprocess-apple-binding.py "$output_root/swift/envoix_ffi.swift"

kotlin_binding="$output_root/kotlin/dev/envoix/app/ffi/envoix_ffi.kt"
swift_binding="$output_root/swift/envoix_ffi.swift"
test -s "$kotlin_binding"
test -s "$swift_binding"
rg -q 'sealed class FfiApplicationCommand' "$kotlin_binding"
rg -q 'sealed class FfiApplicationEvent' "$kotlin_binding"
rg -q 'data class FfiApplicationSnapshot' "$kotlin_binding"
rg -q 'sealed class FfiRoomControlException' "$kotlin_binding"
rg -q 'class Rejected' "$kotlin_binding"
rg -q 'class NetworkLost' "$kotlin_binding"
rg -q 'class Canceled' "$kotlin_binding"
rg -q 'class Failed' "$kotlin_binding"
rg -q 'data class FfiStagedProviderRootV2' "$kotlin_binding"
rg -q 'enum class FfiProviderSourceIssueKindV2' "$kotlin_binding"
rg -q 'suspend fun `addStagedProviderRoots`' "$kotlin_binding"
rg -q 'suspend fun `reauthorizeStagedProviderSource`' "$kotlin_binding"
rg -q 'interface ManifestV2PlatformDestination' "$kotlin_binding"
rg -q 'interface FfiLogSink' "$kotlin_binding"
rg -q 'interface FfiRememberedCredentialVault' "$kotlin_binding"
rg -q 'data class FfiDestinationPlanRequestV2' "$kotlin_binding"
rg -q 'data class FfiDestinationCommitRequestV2' "$kotlin_binding"
rg -q 'suspend fun `receiveWithPlatformDestination`' "$kotlin_binding"
rg -q 'fun `initLogging`' "$kotlin_binding"
rg -q 'fun `setLogLevel`' "$kotlin_binding"
rg -q 'fun `storePairingCredential`' "$kotlin_binding"
if awk '/data class FfiRoomControlSnapshot \(/{capture=1} capture{print} capture && /^\)/{exit}' \
    "$kotlin_binding" | rg -q 'pairingCredential'; then
  echo "error: Kotlin room snapshot exposes a pairing credential" >&2
  exit 1
fi
if awk '/public interface TransferObserver/{capture=1} capture{print} capture && /^}/{exit}' \
    "$kotlin_binding" | rg -q 'onRememberedCredential'; then
  echo "error: Kotlin transfer observer exposes a remembered credential" >&2
  exit 1
fi
if rg -q 'suspend fun `close`\(\)' "$kotlin_binding"; then
  echo "error: async close() conflicts with UniFFI AutoCloseable.close() in Kotlin" >&2
  exit 1
fi
rg -q 'public enum FfiApplicationCommand' "$swift_binding"
rg -q 'public enum FfiApplicationEvent' "$swift_binding"
rg -q 'public struct FfiApplicationSnapshot' "$swift_binding"
rg -q 'public enum FfiRoomControlError' "$swift_binding"
rg -q 'case Rejected' "$swift_binding"
rg -q 'case NetworkLost' "$swift_binding"
rg -q 'case Canceled' "$swift_binding"
rg -q 'case Failed' "$swift_binding"
rg -q 'public struct FfiStagedProviderRootV2' "$swift_binding"
rg -q 'public enum FfiProviderSourceIssueKindV2' "$swift_binding"
rg -q 'func addStagedProviderRoots' "$swift_binding"
rg -q 'func reauthorizeStagedProviderSource' "$swift_binding"
rg -q 'protocol ManifestV2PlatformDestination' "$swift_binding"
rg -q 'protocol FfiLogSink' "$swift_binding"
rg -q 'protocol FfiRememberedCredentialVault' "$swift_binding"
rg -q 'struct FfiDestinationPlanRequestV2' "$swift_binding"
rg -q 'struct FfiDestinationCommitRequestV2' "$swift_binding"
rg -q 'func receiveWithPlatformDestination' "$swift_binding"
rg -q 'func initLogging' "$swift_binding"
rg -q 'func setLogLevel' "$swift_binding"
rg -q 'func storePairingCredential' "$swift_binding"
if awk '/public struct FfiRoomControlSnapshot:/{capture=1} capture{print} capture && /^}/{exit}' \
    "$swift_binding" | rg -q 'pairingCredential'; then
  echo "error: Swift room snapshot exposes a pairing credential" >&2
  exit 1
fi
if awk '/public protocol TransferObserver:/{capture=1} capture{print} capture && /^}/{exit}' \
    "$swift_binding" | rg -q 'onRememberedCredential'; then
  echo "error: Swift transfer observer exposes a remembered credential" >&2
  exit 1
fi

echo "Swift and Kotlin typed application bindings generated successfully."
