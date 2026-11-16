default:
    @just --list

app_bundle := "target/release/bundle/macos/Packrat.app"

# Build Packrat.app
build:
    cd app && cargo tauri build

# Run from source as "Packrat Dev", which keeps its own data and permissions
run:
    cd app && cargo tauri dev --config src-tauri/tauri.dev.conf.json

# Build and copy Packrat.app into /Applications
install: build
    rm -rf /Applications/Packrat.app
    cp -R {{app_bundle}} /Applications/

# Build Packrat.app as the given version and zip it into dist/, as the release workflow does
package version:
    cd app && cargo tauri build --config '{"version":"{{version}}"}'
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
