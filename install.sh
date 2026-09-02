#!/bin/bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
config_dir=${XDG_CONFIG_HOME:-"$HOME/.config"}/zen-tab-launcher
bin_dir="$HOME/.local/bin"
native_dir="$HOME/.mozilla/native-messaging-hosts"
manifest="$native_dir/app.zen_tab_launcher.json"
cargo_bin=${CARGO:-"$HOME/.cargo/bin/cargo"}

mkdir -p "$config_dir" "$bin_dir" "$native_dir"
if [[ ! -f "$config_dir/config.json" ]]; then
  cp "$root/config.example.json" "$config_dir/config.json"
fi
"$cargo_bin" build --release --manifest-path "$root/Cargo.toml"
install -m 755 "$root/target/release/zen-tab" "$bin_dir/zen-tab"
sed "s|@ZEN_TAB_PATH@|$bin_dir/zen-tab|g" \
  "$root/native/app.zen_tab_launcher.json.in" >"$manifest"
chmod 755 "$root/scripts/build-extension.sh"
"$root/scripts/build-extension.sh"
"$bin_dir/zen-tab" sync-targets

echo "Installed the native host, zen-tab command, configuration, and target launcher entries."
echo "Load $root/extension/manifest.json from about:debugging#/runtime/this-firefox to test the extension."
echo "The packaged extension is at $root/build/zen-tab-launcher.xpi."
