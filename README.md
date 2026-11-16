# Packrat

Clipboard history in the macOS menu bar. Press a hotkey, filter, press Enter to paste.

Packrat is the successor to [clippy](https://github.com/bvdwalt/clippy), a terminal clipboard manager written in Go.

## Status

Early development, not ready for daily use.

Working today:

- Text clipboard history with pinning, fuzzy search and expiry of unpinned clips
- Skips secrets copied from password managers
- History encrypted at rest, with the key in the macOS Keychain
- Popup on Cmd+Shift+V that pastes the chosen clip into the app you were using
- Settings that apply as soon as you save them

Still to come:

- Signed releases and launch at login

## Pasting

Enter pastes the selected clip by sending Cmd+V to the app you came from. macOS only allows that once you add Packrat under System Settings > Privacy & Security > Accessibility. Until then Enter copies the clip instead, and the popup shows an "Allow pasting…" link that opens the right pane. Cmd+Enter always copies without pasting.

## Settings

Packrat writes a commented `config.toml` on first run, at `~/Library/Application Support/com.jigsmiths.packrat/config.toml`:

```toml
ttl = "24h"             # how long unpinned clips are kept, "0" keeps them forever
hotkey = "Cmd+Shift+V"  # the global hotkey that opens Packrat
```

Edits take effect within a second. If a value is invalid, Packrat keeps the previous settings and shows the reason in its menu bar menu.

## Requirements

- macOS
- Rust (stable), via [rustup](https://rustup.rs)
- [just](https://github.com/casey/just)
- Tauri CLI: `cargo install tauri-cli --version "^2" --locked`

## Development

```bash
just run       # run from source as "Packrat Dev"
just check     # format check, clippy and tests, same as CI
just build     # build target/release/bundle/macos/Packrat.app
just install   # build and copy Packrat.app into /Applications
```

`just run` uses its own bundle identifier (`com.jigsmiths.packrat.dev`), so a dev build never shares data or permissions with an installed Packrat.

## Layout

- `crates/packrat-core`: storage, history, search and pasteboard access
- `app/src-tauri`: the menu bar app (Tauri 2)
- `app/ui`: the popup frontend (plain HTML and JS)

## License

[MIT](LICENSE)
