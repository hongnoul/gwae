# strimux developer + install targets.

BIN    := target/release/strimux
CARGO  ?= cargo

.PHONY: build install check test clean

## Build the optimised release binary.
build:
	$(CARGO) build --release

## Install the release binary into the first writable `bin` dir on PATH
## (falling back to ~/.local/bin), so `strimux` is runnable immediately even
## when ~/.cargo/bin is not on PATH.
install: build
	@dir="$${PREFIX:-}"; \
	if [ -z "$$dir" ]; then \
		for d in $$(printf '%s' "$$PATH" | tr ':' '\n'); do \
			case "$$d" in *bin) \
				if [ -w "$$d" ] || mkdir -p "$$d" 2>/dev/null; then dir="$$d"; break; fi;; \
			esac; \
		done; \
		[ -n "$$dir" ] || dir="$$(HOME=$$HOME; echo $$HOME/.local/bin)"; \
	fi; \
	mkdir -p "$$dir"; \
	install -m755 $(BIN) "$$dir/strimux"; \
	echo "installed strimux -> $$dir/strimux"

## Lint the whole workspace.
check:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

## Build the core dylib (for hot reload) and the release binary.
core:
	$(CARGO) build -p strimux-core

## Run the hot-reload host `strimux-hmr` (loads target/debug/libstrimux_core).
hmr: core
	$(CARGO) build -p strimux --bin strimux-hmr
	$(CARGO) run -p strimux --bin strimux-hmr

## Watch crates/strimux-core/src and rebuild the core dylib on every save, so
## a running `strimux-hmr` hot-reloads. Run this in one pane and
## `make hmr` (or `strimux-hmr`) in another.
dev-hmr:
	./scripts/develop-hmr.sh

## Run all workspace tests.
test:
	$(CARGO) test --workspace

clean:
	$(CARGO) clean
