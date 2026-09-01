# gwae developer + install targets.

BIN    := target/release/gwae
CARGO  ?= cargo

# Where the user's config lives (same resolution as Config::default_path()).
CONFIG_DIR  := $(if $(XDG_CONFIG_HOME),$(XDG_CONFIG_HOME)/gwae,$(HOME)/.config/gwae)
CONFIG_FILE := $(CONFIG_DIR)/gwae.toml

.PHONY: build install install-keep reset-config check test clean hot dev hot-release

## Build the optimised release binary.
build:
	$(CARGO) build --release

## npm run dev equivalent: one command, async hot reload.
## Builds debug (fast), starts watcher in background, and runs gwae
## with GWAE_DEV_RELOAD=1 so saving a file swaps the binary in place
## without losing any pane (same pid, same PTYs, jcode keeps running).
##   make hot          # debug, ~5s rebuild (default)
##   make hot-release  # release, ~60s rebuild, prod-like
hot:
	@GWAE_PROFILE=debug ./scripts/hot.sh --run

dev: hot

hot-release:
	@GWAE_PROFILE=release ./scripts/hot.sh --run

## Install the release binary into the first writable `bin` dir on PATH
## (falling back to ~/.local/bin), so `gwae` is runnable immediately even
## when ~/.cargo/bin is not on PATH.
##
## Installing also clears any saved preferences (backed up, never deleted) so
## the very next `gwae` run replays the full onboarding / agent gateway
## flow. That is the point during development: the flow is only checkable from
## a machine that has never been onboarded. Use `make install-keep` (or
## `KEEP_CONFIG=1`) to install without touching the config.
install: build $(if $(KEEP_CONFIG),,reset-config)
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
	tmp="$$dir/gwae.new"; \
	cp "$(BIN)" "$$tmp"; \
	chmod 755 "$$tmp"; \
	if command -v codesign >/dev/null 2>&1; then \
		codesign -f -s - "$$tmp" >/dev/null 2>&1 || true; \
	fi; \
	mv -f "$$tmp" "$$dir/gwae"; \
	echo "installed gwae -> $$dir/gwae (atomic: cp .new -> codesign -> mv)"

## Install without clearing preferences.
install-keep:
	@$(MAKE) install KEEP_CONFIG=1

## Move any existing config aside so the next run is a genuine first run.
## The old file is kept as `gwae.toml.bak.<timestamp>`; nothing is deleted.
reset-config:
	@if [ -e "$(CONFIG_FILE)" ]; then \
		bak="$(CONFIG_FILE).bak.$$(date +%Y%m%d%H%M%S)"; \
		mv "$(CONFIG_FILE)" "$$bak"; \
		echo "cleared preferences: $(CONFIG_FILE) -> $$bak"; \
	else \
		echo "no preferences at $(CONFIG_FILE); already a first run"; \
	fi

## Lint the whole workspace.
check:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

## Run all workspace tests.
test:
	$(CARGO) test --workspace

clean:
	$(CARGO) clean
