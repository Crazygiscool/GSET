#!/usr/bin/env bash
# Compile-validates the Windows installer sources without a Windows machine,
# so regressions are caught on every push instead of at release-tag time.
#
#   * gset.nsi - compiled with NSIS 3 (the "nsis" package on Debian/Ubuntu,
#     or any makensis 3 on PATH). A stub gset.exe stands in for the payload;
#     NSIS only embeds the bytes.
#   * gset.wxs - checked for well-formed XML. Full WiX schema validation needs
#     the Wisx toolset (dotnet tool) on a .NET-capable host.
#
# Usage: packages/windows/check-installers.sh
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

if ! command -v makensis >/dev/null 2>&1; then
  echo "makensis not found - install the 'nsis' package (or put makensis on PATH)" >&2
  exit 1
fi

: > "$tmp/gset.exe"
cp "$root/LICENSE" "$tmp/"

makensis -DVERSION=0.0.0-check \
  "-DBINARY=$tmp/gset.exe" \
  "-DLICENSE=$tmp/LICENSE" \
  "-DICON=$dir/gset.ico" \
  "-DOUT=$tmp/out.exe" \
  "$dir/gset.nsi"

python3 -c "import xml.dom.minidom; xml.dom.minidom.parse('$dir/gset.wxs')"

echo "Windows installer sources OK"