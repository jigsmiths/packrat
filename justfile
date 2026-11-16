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
