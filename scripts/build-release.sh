#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${1:-${ROOT}/dist/release}"
BUILD_DIR="${RHIZOME_BUILD_DIR:-${ROOT}/build/pyinstaller}"
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

uv run --group freeze pyinstaller \
  --noconfirm \
  --onefile \
  --clean \
  --paths "${ROOT}/src" \
  --collect-submodules rhizome \
  --collect-submodules gnomon \
  --collect-submodules orrery_heartbeat \
  --name rhizome \
  --distpath "${BUILD_DIR}/dist" \
  --workpath "${BUILD_DIR}/work/rhizome" \
  --specpath "${BUILD_DIR}/spec" \
  "${ROOT}/scripts/rhizome_entry.py"

install -m 0755 \
  "${BUILD_DIR}/dist/rhizome" \
  "${OUTPUT_DIR}/rhizome-${platform}-${arch}"
if [[ "${SKIP_SMOKE:-0}" != "1" ]]; then
  smoke_root="$(mktemp -d)"
  CI=1 ORRERY_NO_UPDATE_CHECK=1 XDG_DATA_HOME="${smoke_root}/data" XDG_CACHE_HOME="${smoke_root}/cache" \
    "${OUTPUT_DIR}/rhizome-${platform}-${arch}" --help >/dev/null
  rm -rf "${smoke_root}"
fi
printf 'built %s binary in %s\n' "${platform}-${arch}" "${OUTPUT_DIR}"
