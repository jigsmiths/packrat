default:
    @just --list

app_bundle := "target/release/bundle/macos/Packrat.app"

notices := "app/src-tauri/THIRD-PARTY-LICENSES.txt"
bundle_config := "--config src-tauri/tauri.bundle.conf.json"

# Build Packrat.app
build: licenses
    cd app && cargo tauri build {{bundle_config}}

# Write the third-party license notices that ship inside Packrat.app
licenses:
    cargo about generate --locked --fail -m app/src-tauri/Cargo.toml -o {{notices}} about.hbs

# Run from source as "Packrat Dev", which keeps its own data and permissions
run:
    cd app && cargo tauri dev --config src-tauri/tauri.dev.conf.json

# Build and copy Packrat.app into /Applications
install: build
    rm -rf /Applications/Packrat.app
    cp -R {{app_bundle}} /Applications/

# Build Packrat.app as the given version and zip it into dist/, as the release workflow does
package version: licenses
    cd app && cargo tauri build {{bundle_config}} --config '{"version":"{{version}}"}'
    mkdir -p dist
    ditto -c -k --keepParent {{app_bundle}} dist/Packrat-{{version}}.zip

# Render the app and menu bar icons from their SVG sources
icons:
    scripts/icons.sh

test:
    cargo test --workspace

# The core is linted on its own too, so the app's dependencies cannot enable features it forgot to declare
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo clippy -p packrat-core --all-targets -- -D warnings

fmt:
    cargo fmt --all

# Fail on a dependency whose license about.toml does not accept
check-licenses:
    cargo about generate --locked --fail -m app/src-tauri/Cargo.toml -o "$(mktemp)" about.hbs

# Clip text never goes through innerHTML, and images load under the CSP
check-ui:
    ! grep -rn innerHTML app/ui
    scripts/check-ui-urls.sh

# What CI runs
check:
    cargo fmt --all --check
    just lint
    just test
    just check-ui
    just check-licenses
