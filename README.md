# Zen Tab Launcher for Omarchy

Search open Zen Browser tabs from Omarchy's app launcher by title, domain, or URL. Press Enter to focus Zen and select the tab.

Tabs use their favicons. Titles stay fixed until the URL changes, which prevents frequently updating sites from repainting the launcher. Saved targets remain searchable when closed and can also be opened from Hyprland keybindings.

## Install

Requires Omarchy, Zen Browser, Rust, `curl`, and `zip`.

```bash
git clone https://github.com/MansTomb/zen-tab-launcher.git
cd zen-tab-launcher
./install.sh
```

In Zen, open `about:debugging#/runtime/this-firefox`, select "Load Temporary Add-on", and choose `extension/manifest.json`.

The extension must be loaded again after restarting Zen until it is signed for permanent installation. Saved targets still open without the extension.

## Saved targets

Edit `~/.config/zen-tab-launcher/config.json`:

```json
{
  "targets": {
    "mail": {
      "name": "Mail",
      "url": "https://mail.example.com/",
      "favicon": "https://mail.example.com/favicon.ico",
      "aliases": ["inbox"],
      "match": "origin",
      "open": "reuse-or-create"
    }
  },
  "zenWindowClass": "zen"
}
```

Apply changes with:

```bash
zen-tab sync-targets
```

`match` accepts `exact` or `origin`. `open` accepts `focus`, `reuse-or-create`, or `always-new`.

## Hyprland keybinding

```lua
o.bind("SUPER + SHIFT + M", "Mail", "zen-tab open mail")
```

Check for conflicts with `omarchy menu keybindings --print`, then validate changes with `hyprctl reload` and `hyprctl configerrors`.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
./scripts/build-extension.sh
```

## Uninstall

```bash
./uninstall.sh
```

Licensed under [MIT](LICENSE).
