#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
system_tmp="${TMPDIR:-/tmp}"
system_tmp="${system_tmp%/}"
repo_key="$(printf '%s' "$repo_root" | cksum | awk '{print $1}')"
minimum_free_gib="${ENVOIX_BUILD_CACHE_MIN_FREE_GIB:-64}"
target_free_gib="${ENVOIX_BUILD_CACHE_TARGET_FREE_GIB:-96}"
mode="${1:---auto}"
mark_path="${2:-}"
cleanup_heartbeat_pid=""

usage() {
  cat <<'EOF'
Usage: scripts/build-cache-guard.sh [--auto|--check|--status|--dry-run]
       scripts/build-cache-guard.sh --mark PATH

  --auto     Below the target, remove regenerable Envoix build/test caches.
             Refuse to start a new build if the hard minimum is not restored.
  --check    Do not delete anything; fail below the hard minimum.
  --status   Print current free space and configured watermarks.
  --dry-run  Show what --auto would remove without changing the filesystem.
  --mark     Mark one top-level envoix-* temporary directory as regenerable.

Environment:
  ENVOIX_BUILD_CACHE_MIN_FREE_GIB     Hard minimum (default: 64)
  ENVOIX_BUILD_CACHE_TARGET_FREE_GIB  Cleanup target (default: 96)
EOF
}

require_positive_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
    echo "error: $name must be a positive integer" >&2
    exit 2
  fi
}

free_kib() {
  df -Pk "$repo_root" | awk 'NR == 2 { print $4 }'
}

gib_from_kib() {
  awk -v kib="$1" 'BEGIN { printf "%.1f", kib / 1048576 }'
}

mtime_epoch() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    stat -f '%m' "$1"
  else
    stat -c '%Y' "$1"
  fi
}

build_process_is_running() {
  pgrep -x xcodebuild >/dev/null 2>&1 \
    || pgrep -x cargo >/dev/null 2>&1 \
    || pgrep -x rustc >/dev/null 2>&1 \
    || pgrep -f 'org\.gradle\.wrapper\.GradleWrapperMain' >/dev/null 2>&1
}

is_regenerable_transient_path() {
  local path="$1"
  local name="${path##*/}"
  [[ -d "$path" ]] || return 1
  case "$name" in
    envoix-apple-cache|envoix-*.xcresult)
      return 0
      ;;
  esac
  [[ -f "$path/.envoix-regenerable-build-cache" ]]
}

disposable_transient_candidates() {
  local root path
  for root in /private/tmp "$system_tmp"; do
    [[ -d "$root" ]] || continue
    while IFS= read -r -d '' path; do
      [[ "$path" == "$lock_dir" ]] && continue
      case "${path##*/}" in
        envoix-apple-cache)
          continue
          ;;
      esac
      is_regenerable_transient_path "$path" || continue
      printf '%s\t%s\n' "$(mtime_epoch "$path")" "$path"
    done < <(find "$root" -mindepth 1 -maxdepth 1 -name 'envoix-*' -print0)
  done | LC_ALL=C sort -n | cut -f 2- | awk '!seen[$0]++'
}

stable_cache_candidates() {
  local path
  for path in \
    /private/tmp/envoix-apple-cache \
    "$system_tmp/envoix-apple-cache"; do
    [[ -d "$path" ]] && printf '%s\n' "$path"
  done | awk '!seen[$0]++'
}

repository_build_candidates() {
  local path
  for path in \
    "$repo_root/apps/envoix-apple/build" \
    "$repo_root/apps/envoix-apple/build-ios-ui-test"; do
    [[ -d "$path" ]] && printf '%s\n' "$path"
  done
}

is_safe_cache_path() {
  local path="$1"
  case "$path" in
    "$repo_root"/target)
      return 0
      ;;
    "$repo_root"/apps/envoix-apple/build|\
    "$repo_root"/apps/envoix-apple/build-ios-ui-test)
      return 0
      ;;
    /private/tmp/envoix-*|"$system_tmp"/envoix-*)
      is_regenerable_transient_path "$path"
      ;;
    *)
      return 1
      ;;
  esac
}

remove_cache_path() {
  local path="$1"
  if ! is_safe_cache_path "$path"; then
    echo "error: refusing to remove unsafe cache path: $path" >&2
    exit 2
  fi
  if [[ "$mode" == "--dry-run" ]]; then
    echo "would remove: $path"
  else
    echo "removing build cache: $path"
    rm -rf -- "$path"
  fi
}

acquire_cleanup_lock() {
  local owner_pid="" modified_at now age
  if mkdir "$lock_dir" 2>/dev/null; then
    printf '%s\n' "$$" > "$lock_dir/owner.pid"
    : > "$lock_dir/heartbeat"
    return 0
  fi

  if [[ -f "$lock_dir/owner.pid" ]]; then
    IFS= read -r owner_pid < "$lock_dir/owner.pid" || true
  fi
  if [[ -f "$lock_dir/heartbeat" ]]; then
    modified_at="$(mtime_epoch "$lock_dir/heartbeat" 2>/dev/null || printf '0')"
  else
    modified_at="$(mtime_epoch "$lock_dir" 2>/dev/null || printf '0')"
  fi
  now="$(date +%s)"
  age=$((now - modified_at))
  if [[ "$owner_pid" =~ ^[1-9][0-9]*$ ]]; then
    if kill -0 "$owner_pid" 2>/dev/null || (( age < 120 )); then
      echo "Build cache guard: cleanup lease is active or recent (pid $owner_pid); refusing to start another build." >&2
      return 1
    fi
  fi
  if [[ -z "$owner_pid" && "$age" -lt 120 ]]; then
    echo "Build cache guard: a recent ownerless cleanup lock exists; refusing to start another build." >&2
    return 1
  fi

  rm -rf -- "$lock_dir"
  if ! mkdir "$lock_dir" 2>/dev/null; then
    echo "Build cache guard: could not replace a stale cleanup lock; refusing to start another build." >&2
    return 1
  fi
  printf '%s\n' "$$" > "$lock_dir/owner.pid"
  : > "$lock_dir/heartbeat"
}

heartbeat_cleanup_lock() {
  local owner_pid=""
  while [[ -d "$lock_dir" ]]; do
    if [[ -f "$lock_dir/owner.pid" ]]; then
      IFS= read -r owner_pid < "$lock_dir/owner.pid" || true
    fi
    [[ "$owner_pid" == "$$" ]] || return 0
    kill -0 "$owner_pid" 2>/dev/null || return 0
    touch "$lock_dir/heartbeat"
    sleep 15
  done
}

release_cleanup_lock() {
  local owner_pid=""
  if [[ -n "$cleanup_heartbeat_pid" ]]; then
    kill "$cleanup_heartbeat_pid" >/dev/null 2>&1 || true
    wait "$cleanup_heartbeat_pid" >/dev/null 2>&1 || true
  fi
  if [[ -f "$lock_dir/owner.pid" ]]; then
    IFS= read -r owner_pid < "$lock_dir/owner.pid" || true
  fi
  if [[ "$owner_pid" == "$$" ]]; then
    rm -rf -- "$lock_dir"
  fi
}

mark_regenerable_cache() {
  local path="$1"
  local parent="${path%/*}"
  local name="${path##*/}"
  if [[ -z "$path" || "$parent" == "$path" || "$name" != envoix-* ]]; then
    echo "error: --mark requires a top-level envoix-* path under /private/tmp or TMPDIR" >&2
    exit 2
  fi
  if [[ "$parent" != "/private/tmp" && "$parent" != "$system_tmp" ]]; then
    echo "error: refusing to mark a cache outside /private/tmp or TMPDIR: $path" >&2
    exit 2
  fi
  if [[ -L "$path" ]]; then
    echo "error: refusing to mark a symbolic link: $path" >&2
    exit 2
  fi
  if [[ -d "$path" && ! -f "$path/.envoix-regenerable-build-cache" ]] \
      && find "$path" -mindepth 1 -maxdepth 1 -print -quit | grep -q .; then
    echo "error: refusing to adopt a non-empty unmarked directory as build cache: $path" >&2
    exit 2
  fi
  mkdir -p "$path"
  : > "$path/.envoix-regenerable-build-cache"
  echo "marked regenerable build cache: $path"
}

if [[ "$mode" == "--mark" ]]; then
  [[ "$#" -eq 2 ]] || { usage >&2; exit 2; }
  mark_regenerable_cache "$mark_path"
  exit 0
fi
if [[ "$#" -gt 1 ]]; then
  usage >&2
  exit 2
fi

require_positive_integer ENVOIX_BUILD_CACHE_MIN_FREE_GIB "$minimum_free_gib"
require_positive_integer ENVOIX_BUILD_CACHE_TARGET_FREE_GIB "$target_free_gib"
if (( target_free_gib <= minimum_free_gib )); then
  echo "error: target free space must exceed the hard minimum" >&2
  exit 2
fi

minimum_free_kib=$((minimum_free_gib * 1024 * 1024))
target_free_kib=$((target_free_gib * 1024 * 1024))
available_kib="$(free_kib)"

case "$mode" in
  --status)
    echo "free=$(gib_from_kib "$available_kib") GiB hard-min=${minimum_free_gib} GiB target=${target_free_gib} GiB"
    exit 0
    ;;
  --check)
    if (( available_kib < minimum_free_kib )); then
      echo "Build cache guard: only $(gib_from_kib "$available_kib") GiB free; below the ${minimum_free_gib} GiB hard minimum." >&2
      exit 4
    fi
    echo "Build cache guard: $(gib_from_kib "$available_kib") GiB free; build products preserved."
    exit 0
    ;;
  --auto|--dry-run)
    ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

if (( available_kib >= target_free_kib )); then
  echo "Build cache guard: $(gib_from_kib "$available_kib") GiB free; no cleanup needed."
  exit 0
fi

lock_dir="$system_tmp/envoix-build-cache-guard-$(id -u)-$repo_key.lock"
lease_base="$system_tmp/envoix-build-$(id -u)-$repo_key"
if [[ "$mode" == "--auto" && "${ENVOIX_BUILD_LEASE_HELD:-0}" != "1" ]]; then
  if [[ -d "$lease_base.writer" ]] \
      || find "$lease_base.readers" -mindepth 1 -maxdepth 1 -type d -print -quit 2>/dev/null | grep -q .; then
    echo "Build cache guard: an Envoix lease is active; run cleanup through the guarded wrapper." >&2
    exit 3
  fi
fi
if ! acquire_cleanup_lock; then
  exit 3
fi
trap release_cleanup_lock EXIT INT TERM
heartbeat_cleanup_lock &
cleanup_heartbeat_pid="$!"

if build_process_is_running; then
  echo "Build cache guard: only $(gib_from_kib "$available_kib") GiB free and another build is active; refusing to start a concurrent build." >&2
  exit 3
fi

echo "Build cache guard: only $(gib_from_kib "$available_kib") GiB free; cleaning toward ${target_free_gib} GiB."
while IFS= read -r path; do
  [[ -n "$path" ]] || continue
  remove_cache_path "$path"
  [[ "$mode" == "--dry-run" ]] && continue
  available_kib="$(free_kib)"
  (( available_kib >= target_free_kib )) && break
done < <(disposable_transient_candidates)

if (( available_kib < target_free_kib )); then
  while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    remove_cache_path "$path"
    [[ "$mode" == "--dry-run" ]] && continue
    available_kib="$(free_kib)"
    (( available_kib >= target_free_kib )) && break
  done < <(repository_build_candidates)
fi

available_kib="$(free_kib)"
if (( available_kib < minimum_free_kib )); then
  while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    remove_cache_path "$path"
    [[ "$mode" == "--dry-run" ]] && continue
    available_kib="$(free_kib)"
    (( available_kib >= minimum_free_kib )) && break
  done < <(stable_cache_candidates)
fi

available_kib="$(free_kib)"
if (( available_kib < minimum_free_kib )) && [[ -d "$repo_root/target" ]]; then
  remove_cache_path "$repo_root/target"
  available_kib="$(free_kib)"
fi

if [[ "$mode" == "--dry-run" ]]; then
  echo "Build cache guard dry run complete."
elif (( available_kib < minimum_free_kib )); then
  echo "Build cache guard: only $(gib_from_kib "$available_kib") GiB free after safe cleanup; refusing to build below the ${minimum_free_gib} GiB hard minimum." >&2
  exit 4
elif (( available_kib < target_free_kib )); then
  echo "Build cache guard: $(gib_from_kib "$available_kib") GiB free; safe cleanup candidates exhausted before the ${target_free_gib} GiB target, but the ${minimum_free_gib} GiB hard minimum is satisfied." >&2
else
  echo "Build cache guard: $(gib_from_kib "$available_kib") GiB free after cleanup."
fi
