# gwae developer + install targets.

BIN    := target/release/gwae
DEV_BIN := target/debug/gwae
CARGO  ?= cargo

# Where the user's config lives (same resolution as Config::default_path()).
CONFIG_DIR  := $(if $(XDG_CONFIG_HOME),$(XDG_CONFIG_HOME)/gwae,$(HOME)/.config/gwae)
CONFIG_FILE := $(CONFIG_DIR)/gwae.toml

.PHONY: build install install-keep reset-config dev dev-build

## Build the optimised release binary.
build:
	$(CARGO) build --release

## Fast debug build for `dev` (no config reset, no install).
dev-build:
	$(CARGO) build
	@if command -v codesign >/dev/null 2>&1; then \
		codesign -f -s - "$(DEV_BIN)" >/dev/null 2>&1 || true; \
	fi

## Launch the hot-reload dev instance: debug binary + GWAE_DEV_RELOAD=1.
## Rebuild anytime with `make dev-build`; the running instance execs into it.
## With WATCH=1 (default) the session rebuilds itself on source change, so
## no manual step is needed: only a clean, signed, loadable binary triggers
## the exec, and failures stay invisible on the last good image.
## On exit the release binary is installed (config-preserving, like
## `install-keep`, never `install`: quitting dev must not wipe onboarding
## preferences) so the `gwae` on PATH is always the latest dev build. The
## release rebuild is incremental, a no-op when nothing changed. Set
## `NO_INSTALL=1` to skip (e.g. quitting a broken intermediate state).
## Set `WATCH=0` for the old manual behavior.
dev: dev-build
	trap 'if [ -z "$${NO_INSTALL:-}" ]; then $(MAKE) install-keep || echo "dev-exit install failed (stable gwae unchanged)"; fi' EXIT; \
	if [ "$${WATCH:-1}" = "1" ]; then export GWAE_DEV_WATCH=1; fi; \
	GWAE_DEV_RELOAD=1 "$(DEV_BIN)" $(ARGS)

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
	echo "installed gwae -> $$dir/gwae (atomic: cp .new -> codesign -> mv)"; \
	state_dir="$${XDG_STATE_HOME:-$$HOME/.local/state}/gwae"; \
	if mkdir -p "$$state_dir" 2>/dev/null; then \
		version="$$($$dir/gwae --version 2>/dev/null || true)"; \
		version="$${version##* }"; \
		{ echo "# Written by gwae's Makefile (make install); read by gwae upgrade. Safe to delete."; \
		  echo 'source = "source"'; \
		  echo "dir = \"$$dir\""; \
		  echo "version = \"$$version\""; \
		} > "$$state_dir/install.toml"; \
	fi

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
