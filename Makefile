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

## Run all workspace tests.
test:
	$(CARGO) test --workspace

clean:
	$(CARGO) clean
