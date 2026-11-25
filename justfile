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

test:
    cargo test --workspace

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo clippy -p packrat-core --all-targets -- -D warnings

fmt:
    cargo fmt --all

# Same checks as CI
check:
    cargo fmt --all --check
    just lint
    just test
    ! grep -rn innerHTML app/ui
    scripts/check-unsafe-sites.sh
