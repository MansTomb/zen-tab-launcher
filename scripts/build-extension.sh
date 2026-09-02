#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p "$root/build"
rm -f "$root/build/zen-tab-launcher.xpi"
cd "$root/extension"
zip -q -r "$root/build/zen-tab-launcher.xpi" manifest.json background.js
echo "$root/build/zen-tab-launcher.xpi"
