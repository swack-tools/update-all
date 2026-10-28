# Lists every recipe.
default:
    @just --list

# Prints update-all's full help.
help:
    @cargo run --release --quiet -- --help

# Runs update-all with any flags, for example `just run --electron`. With no flags, prints help.
[positional-arguments]
run *args:
    @if [ "$#" -eq 0 ]; then cargo run --release --quiet -- --help; else cargo run --release --quiet -- "$@"; fi

# Previews updates without installing anything. Accepts extra flags too.
[positional-arguments]
dry-run *args:
    @cargo run --release --quiet -- --dry-run "$@"

# Runs formatting, clippy, tests, and Vale.
check: fmt clippy test vale

# Shows the newest run log.
log:
    @cat "$(ls -t ~/Library/Logs/update-all/*.log | head -1)"

# Builds the optimized binary at target/release/update-all.
release:
    cargo build --release

# Installs the binary into ~/.cargo/bin.
install:
    cargo install --path .

# Removes build output.
clean:
    cargo clean

# Checks formatting without changing files.
fmt:
    cargo fmt --check

# Formats the code in place.
format:
    cargo fmt

# Fails on any clippy warning.
clippy:
    cargo clippy --all-targets -- -D warnings

# Runs the tests. One test exports every user-facing string to target/vale/.
test:
    cargo test

# Checks all prose against the Google style and fails on any finding, even a suggestion.
vale:
    vale sync
    cargo test --quiet copy_passes_vale
    @out="$(vale --output=line *.md .github src target/vale)"; if [ -n "$out" ]; then echo "$out"; exit 1; fi; echo "Vale: no errors, warnings, or suggestions."
