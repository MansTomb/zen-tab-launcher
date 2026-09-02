# Zen Tab Launcher

Your app launcher already knows how to find applications. Zen Tab Launcher teaches it about browser tabs.

Type a tab title, domain, or URL in Omarchy's launcher and press Enter. Zen comes forward with that tab selected. Results use the site's favicon and stay still when a site changes its title in the background.

Configured targets remain searchable when closed. They can focus an existing match, open one when needed, or always create a fresh tab. The same targets work in Hyprland keybindings.

## What you get

- Live Zen tabs beside native applications
- Search by title, URL, and domain
- Real favicons with a Zen icon fallback
- Stable titles for noisy sites such as dashboards and trading pages
- Saved targets for sites that are not currently open
- A safe URL fallback when the extension is disconnected
- One 1.3 MB Rust binary for the CLI and native host

## Install

You need Rust, Zen Browser, Omarchy, `curl`, and `zip`.

```bash
git clone https://github.com/MansTomb/zen-tab-launcher.git
cd zen-tab-launcher
./install.sh
```

Open `about:debugging#/runtime/this-firefox` in Zen, select "Load Temporary Add-on", and choose `extension/manifest.json`.

Firefox-derived release browsers require signed extensions for permanent installation. Until the extension is signed, reload it from `about:debugging` after restarting Zen. Saved targets and stale live entries still open their URLs when the extension is unavailable.

## Configure targets

The installer creates `~/.config/zen-tab-launcher/config.json` on first run:

```json
{
  "targets": {
    "mail": {
      "name": "Mail",
      "url": "https://mail.example.com/",
      "favicon": "https://mail.example.com/favicon.ico",
      "aliases": ["mail", "inbox"],
      "match": "origin",
      "open": "reuse-or-create"
    }
  },
  "zenWindowClass": "zen"
}
```

After editing it, refresh the saved entries:

```bash
zen-tab sync-targets
```

`match` accepts `exact` or `origin`. `open` accepts `focus`, `reuse-or-create`, or `always-new`. The optional `favicon` must be an HTTP or HTTPS image URL. The optional `container` matches a Firefox container by name, which Zen can associate with an internal workspace.

## Add a keybinding

Check your existing bindings first:

```bash
omarchy menu keybindings --print
```

Then add a target to `~/.config/hypr/bindings.lua`:

```lua
o.bind("SUPER + SHIFT + M", "Mail", "zen-tab open mail")
```

If the shortcut already exists, unbind it before assigning the replacement. Validate the result with:

```bash
hyprctl reload
hyprctl configerrors
```

## How it works

The Zen extension sends a compact tab snapshot to the Rust native host. The host writes namespaced `.desktop` entries under `~/.local/share/applications`, which Omarchy already watches. Selecting one sends its tab ID back through a private Unix socket. The extension activates the tab, then Hyprland focuses Zen's window.

Titles and favicons are captured once per URL. Background title changes do not rewrite launcher entries. Opening, closing, or navigating a tab updates the relevant entry.

The project never edits `/usr/share/omarchy`. Cleanup only touches files prefixed with `zen-tab-live-` or `zen-tab-target-`.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
./scripts/build-extension.sh
```

## Uninstall

Unload the extension, then run:

```bash
./uninstall.sh
```

The uninstaller keeps your configuration.

## License

[MIT](LICENSE)
