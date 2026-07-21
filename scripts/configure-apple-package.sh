#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
package_file="${1:-$repo_root/crates/envoix-ffi/EnvoixCore/Package.swift}"

if [[ ! -f "$package_file" ]]; then
  echo "error: generated Apple package not found at $package_file" >&2
  exit 2
fi
if grep -Fq 'linkedFramework("SystemConfiguration")' "$package_file"; then
  exit 0
fi

patched_file="${package_file}.patched"
awk '
  /^[[:space:]]*\.target\(/ { in_target = 1 }
  in_target && /^[[:space:]]*dependencies:/ { in_dependencies = 1 }
  in_target && in_dependencies && /^[[:space:]]*\][[:space:]]*$/ {
    sub(/\][[:space:]]*$/, "],")
    print
    print "            linkerSettings: ["
    print "                .linkedFramework(\"SystemConfiguration\"),"
    print "                .linkedFramework(\"Network\"),"
    print "                .linkedFramework(\"Security\"),"
    print "            ]"
    in_dependencies = 0
    next
  }
  { print }
  in_target && /^[[:space:]]*\),$/ { in_target = 0 }
' "$package_file" > "$patched_file"
mv "$patched_file" "$package_file"

if ! grep -Fq 'linkedFramework("SystemConfiguration")' "$package_file"; then
  echo "error: failed to configure Apple package linker settings" >&2
  exit 1
fi
