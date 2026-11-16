default:
    @just --list

# Build the release binary with the frontend embedded
build:
    cargo build --release -p packrat --features custom-protocol

# Run the app from source
run:
    cargo run -p packrat --features custom-protocol

test:
    cargo test --workspace

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt:
    cargo fmt --all

# Same checks as CI
check:
    cargo fmt --all --check
    just lint
    just test
