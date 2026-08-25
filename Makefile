# gwae developer + install targets.

BIN    := target/release/gwae
CARGO  ?= cargo

# Where the user's config lives (same resolution as Config::default_path()).
CONFIG_DIR  := $(if $(XDG_CONFIG_HOME),$(XDG_CONFIG_HOME)/gwae,$(HOME)/.config/gwae)
CONFIG_FILE := $(CONFIG_DIR)/gwae.toml

.PHONY: build install install-keep reset-config check test clean

## Build the optimised release binary.
build:
	$(CARGO) build --release

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
	install -m755 $(BIN) "$$dir/gwae"; \
	echo "installed gwae -> $$dir/gwae"

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
