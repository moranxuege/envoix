#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
system_tmp="${TMPDIR:-/tmp}"
system_tmp="${system_tmp%/}"
repo_key="$(printf '%s' "$repo_root" | cksum | awk '{print $1}')"
lease_base="$system_tmp/envoix-build-$(id -u)-$repo_key"
writer_dir="$lease_base.writer"
reader_root="$lease_base.readers"
lease_dir=""
heartbeat_pid=""

usage() {
  cat >&2 <<'EOF'
Usage: scripts/with-build-cache-guard.sh [options] <command> [arguments...]

Options:
  --preserve-build-products  Take a shared read lease, do not delete products,
                             and fail below the hard free-space minimum.
  --cache-path PATH          After cleanup, mark a dedicated temporary cache as
                             regenerable.
EOF
}

mtime_epoch() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    stat -f '%m' "$1"
  else
    stat -c '%Y' "$1"
  fi
}

lease_is_active_or_recent() {
  local path="$1"
  local owner_pid="" timestamp now age
  [[ -d "$path" ]] || return 1
  if [[ -f "$path/owner.pid" ]]; then
    IFS= read -r owner_pid < "$path/owner.pid" || true
  fi
  if [[ "$owner_pid" =~ ^[1-9][0-9]*$ ]] && kill -0 "$owner_pid" 2>/dev/null; then
    return 0
  fi
  if [[ -f "$path/heartbeat" ]]; then
    timestamp="$(mtime_epoch "$path/heartbeat" 2>/dev/null || printf '0')"
  else
    timestamp="$(mtime_epoch "$path" 2>/dev/null || printf '0')"
  fi
  now="$(date +%s)"
  age=$((now - timestamp))
  (( age < 120 ))
}

remove_stale_lease() {
  local path="$1"
  if lease_is_active_or_recent "$path"; then
    return 1
  fi
  rm -rf -- "$path"
}

acquire_writer_lease() {
  local reader active_reader=0
  if [[ -d "$writer_dir" ]] && ! remove_stale_lease "$writer_dir"; then
    echo "Build lease: another Envoix writer is active." >&2
    return 1
  fi
  if ! mkdir "$writer_dir" 2>/dev/null; then
    echo "Build lease: could not acquire the Envoix writer lease." >&2
    return 1
  fi
  lease_dir="$writer_dir"
  printf '%s\n' "$$" > "$lease_dir/owner.pid"
  : > "$lease_dir/heartbeat"

  if [[ -d "$reader_root" ]]; then
    while IFS= read -r -d '' reader; do
      if lease_is_active_or_recent "$reader"; then
        active_reader=1
      else
        rm -rf -- "$reader"
      fi
    done < <(find "$reader_root" -mindepth 1 -maxdepth 1 -type d -print0)
  fi
  if [[ "$active_reader" == "1" ]]; then
    rm -rf -- "$lease_dir"
    lease_dir=""
    echo "Build lease: a build-preserving test is active; refusing cache mutation." >&2
    return 1
  fi
}

acquire_reader_lease() {
  if [[ -d "$writer_dir" ]] && ! remove_stale_lease "$writer_dir"; then
    echo "Build lease: an Envoix writer is active; build products cannot be read safely." >&2
    return 1
  fi
  mkdir -p "$reader_root"
  lease_dir="$reader_root/reader-$$-$RANDOM"
  if ! mkdir "$lease_dir" 2>/dev/null; then
    echo "Build lease: could not acquire an Envoix reader lease." >&2
    return 1
  fi
  printf '%s\n' "$$" > "$lease_dir/owner.pid"
  : > "$lease_dir/heartbeat"

  if [[ -d "$writer_dir" ]]; then
    rm -rf -- "$lease_dir"
    lease_dir=""
    echo "Build lease: an Envoix writer raced with this reader; retry later." >&2
    return 1
  fi
}

heartbeat_lease() {
  local owner_pid=""
  while [[ -n "$lease_dir" && -d "$lease_dir" ]]; do
    if [[ -f "$lease_dir/owner.pid" ]]; then
      IFS= read -r owner_pid < "$lease_dir/owner.pid" || true
    fi
    [[ "$owner_pid" == "$$" ]] || return 0
    kill -0 "$owner_pid" 2>/dev/null || return 0
    touch "$lease_dir/heartbeat"
    sleep 15
  done
}

release_lease() {
  local owner_pid=""
  if [[ -n "$heartbeat_pid" ]]; then
    kill "$heartbeat_pid" >/dev/null 2>&1 || true
    wait "$heartbeat_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$lease_dir" && -f "$lease_dir/owner.pid" ]]; then
    IFS= read -r owner_pid < "$lease_dir/owner.pid" || true
  fi
  if [[ -n "$lease_dir" && "$owner_pid" == "$$" ]]; then
    rm -rf -- "$lease_dir"
  fi
  rmdir "$reader_root" >/dev/null 2>&1 || true
}

preserve_build_products=0
cache_path=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --preserve-build-products)
      preserve_build_products=1
      shift
      ;;
    --cache-path)
      [[ "$#" -ge 2 ]] || { usage; exit 2; }
      [[ -z "$cache_path" ]] || {
        echo "Build lease: --cache-path may be provided only once." >&2
        exit 2
      }
      cache_path="$2"
      shift 2
      ;;
    --)
      shift
      break
      ;;
    *)
      break
      ;;
  esac
done

if [[ "$#" -eq 0 ]]; then
  usage
  exit 2
fi

if [[ "$preserve_build_products" == "1" && -n "$cache_path" ]]; then
  echo "Build lease: --cache-path cannot be used with --preserve-build-products." >&2
  exit 2
fi

if [[ "${ENVOIX_BUILD_LEASE_HELD:-0}" == "1" ]]; then
  if [[ -n "$cache_path" ]]; then
    echo "Build lease: a nested wrapper cannot introduce a new --cache-path." >&2
    exit 2
  fi
  if [[ "${ENVOIX_BUILD_LEASE_MODE:-writer}" == "reader" && "$preserve_build_products" == "0" ]]; then
    echo "Build lease: a reader command cannot start a cache-mutating child." >&2
    exit 3
  fi
  exec "$@"
fi

if [[ "$preserve_build_products" == "1" ]]; then
  acquire_reader_lease || exit 3
  export ENVOIX_BUILD_LEASE_MODE=reader
else
  acquire_writer_lease || exit 3
  export ENVOIX_BUILD_LEASE_MODE=writer
fi
export ENVOIX_BUILD_LEASE_HELD=1
trap release_lease EXIT INT TERM
heartbeat_lease &
heartbeat_pid="$!"

if [[ "$preserve_build_products" == "0" ]]; then
  "$script_dir/build-cache-guard.sh" --auto
  if [[ -n "$cache_path" ]]; then
    "$script_dir/build-cache-guard.sh" --mark "$cache_path"
  fi
else
  "$script_dir/build-cache-guard.sh" --check
fi
"$@"
