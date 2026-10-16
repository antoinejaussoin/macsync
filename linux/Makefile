.PHONY: build release test linux-amd64 clean

TARGET_LINUX ?= x86_64-unknown-linux-gnu

# macOS has no GNU ld; use zig as the Linux cross linker.
ifeq ($(shell uname -s),Darwin)
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER := $(CURDIR)/scripts/zig-cc
endif

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

# Cross-compile (or native build) for Ubuntu/amd64.
#   rustup target add x86_64-unknown-linux-gnu
# On macOS: brew install zig  (Makefile points cargo at scripts/zig-cc)
linux-amd64:
	@if [ "$$(uname -s)" = Darwin ] && ! command -v zig >/dev/null 2>&1; then \
		echo "error: zig is required to cross-compile from macOS."; \
		echo "  brew install zig"; \
		exit 1; \
	fi
	cargo build --release --target $(TARGET_LINUX)

clean:
	cargo clean
