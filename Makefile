PREFIX ?= $(HOME)
CARGO  ?= cargo

.PHONY: build release test lint fmt fmt-check check run run-mcp install clean

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

# The terminal UI alone.
run: build
	./crates/target/debug/commander $(DIR)

# The terminal UI, also serving MCP on 127.0.0.1:8740/mcp.
run-mcp: build
	./crates/target/debug/commander --mcp $(DIR)

install: release
	install -d $(PREFIX)/bin
	install -m 755 crates/target/release/commander $(PREFIX)/bin/hbui-commander
	@echo "installed $(PREFIX)/bin/hbui-commander"

clean:
	cd crates && $(CARGO) clean
