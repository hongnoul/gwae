# strimux developer + install targets.

PREFIX ?= $(HOME)/.cargo/bin
BIN    := target/release/strimux
CARGO  ?= cargo

.PHONY: build install check test clean

## Build the optimised release binary.
build:
	$(CARGO) build --release

## Install the release binary into $(PREFIX) (default ~/.cargo/bin).
install: build
	install -d $(PREFIX)
	install -m755 $(BIN) $(PREFIX)/strimux

## Lint the whole workspace.
check:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

## Run all workspace tests.
test:
	$(CARGO) test --workspace

clean:
	$(CARGO) clean
