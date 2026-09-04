#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${1:-${ROOT}/dist/release}"
BUILD_DIR="${MEMEX_BUILD_DIR:-${ROOT}/build/pyinstaller}"
export PYINSTALLER_CONFIG_DIR="${PYINSTALLER_CONFIG_DIR:-${BUILD_DIR}/cache}"

case "$(uname -s)" in
  Darwin) platform="darwin" ;;
  Linux) platform="linux" ;;
  *) printf 'unsupported operating system: %s\n' "$(uname -s)" >&2; exit 2 ;;
esac

case "$(uname -m)" in
  arm64|aarch64) arch="arm64" ;;
  x86_64|amd64) arch="x86_64" ;;
  *) printf 'unsupported architecture: %s\n' "$(uname -m)" >&2; exit 2 ;;
esac

mkdir -p "${OUTPUT_DIR}" "${BUILD_DIR}/dist" "${BUILD_DIR}/work" "${BUILD_DIR}/spec" "${PYINSTALLER_CONFIG_DIR}"

build_binary() {
  local name="$1"
  local entry="$2"
  uv run --group freeze pyinstaller \
    --noconfirm \
    --onedir \
    --clean \
    --paths "${ROOT}/src" \
    --collect-all jieba \
    --collect-all bm25s \
    --collect-submodules memex \
    --collect-submodules gnomon \
    --name "${name}" \
    --distpath "${BUILD_DIR}/dist" \
    --workpath "${BUILD_DIR}/work/${name}" \
    --specpath "${BUILD_DIR}/spec" \
    "${ROOT}/${entry}"
  tar -C "${BUILD_DIR}/dist" \
    -czf "${OUTPUT_DIR}/${name}-${platform}-${arch}.tar.gz" "${name}"
}

build_binary "memex" "scripts/memex_entry.py"
build_binary "memex-sync" "scripts/memex_sync_entry.py"

if [[ "${SKIP_SMOKE:-0}" != "1" ]]; then
  smoke_root="$(mktemp -d)"
  CI=1 XDG_DATA_HOME="${smoke_root}/data" XDG_CACHE_HOME="${smoke_root}/cache" \
    "${BUILD_DIR}/dist/memex/memex" --help >/dev/null
  CI=1 XDG_DATA_HOME="${smoke_root}/data" XDG_CACHE_HOME="${smoke_root}/cache" \
    "${BUILD_DIR}/dist/memex-sync/memex-sync" --help >/dev/null
  rm -rf "${smoke_root}"
fi
printf 'built %s binaries in %s\n' "${platform}-${arch}" "${OUTPUT_DIR}"
