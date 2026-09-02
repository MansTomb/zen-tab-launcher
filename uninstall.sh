#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
bin_path="$HOME/.local/bin/zen-tab"
native_manifest="$HOME/.mozilla/native-messaging-hosts/app.zen_tab_launcher.json"

if [[ -x "$bin_path" ]]; then
  "$bin_path" clear-entries || true
fi
if [[ -f "$bin_path" ]]; then
  rm "$bin_path"
fi
if [[ -f "$native_manifest" ]] && grep -Fq "$bin_path" "$native_manifest"; then
  rm "$native_manifest"
fi

echo "Removed Zen Tab Launcher integration files."
echo "The repository and ~/.config/zen-tab-launcher/config.json were kept."
