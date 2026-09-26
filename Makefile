PREFIX ?= $(HOME)
CARGO  ?= cargo

.PHONY: build release test lint fmt fmt-check check bridge run install clean

build:
	cd crates && $(CARGO) build

release:
	cd crates && $(CARGO) build --release

test:
	cd crates && $(CARGO) test

lint:
	cd crates && $(CARGO) clippy --all-targets -- -D warnings

# Separate from `lint` so that a formatting slip and a real lint failure are
# two different red builds. `cargo fmt` with no arguments fixes every one.
fmt:
	cd crates && $(CARGO) fmt

fmt-check:
	cd crates && $(CARGO) fmt --check

# What CI runs, and what to run before a commit.
check: test lint fmt-check

DIR ?= .

# The long-lived half: MCP on 127.0.0.1:8740/mcp. Leave it running.
bridge: build
	./crates/target/debug/hbui-mcp-bridge --http

# The short-lived half: restart it as often as you like; it reconnects.
run: build
	./crates/target/debug/commander $(DIR)

install: release
	install -d $(PREFIX)/bin
	install -m 755 crates/target/release/hbui-mcp-bridge $(PREFIX)/bin/hbui-mcp-bridge
	install -m 755 crates/target/release/commander $(PREFIX)/bin/hbui-commander
	@echo "installed $(PREFIX)/bin/hbui-mcp-bridge and $(PREFIX)/bin/hbui-commander"

clean:
	cd crates && $(CARGO) clean
