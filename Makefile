HOST ?= mini
REMOTE_DIR ?= /Users/antoine/macsync
APP := dist/MacSync.app

.PHONY: deploy bundle install test lint

deploy: bundle
	ssh $(HOST) 'mkdir -p "$(REMOTE_DIR)"'
	rsync -a --delete -e ssh $(APP) $(HOST):"$(REMOTE_DIR)/"
	@echo "Copied MacSync.app to $(HOST):$(REMOTE_DIR)"

bundle:
	scripts/bundle.sh

install:
	scripts/bundle.sh --install

test:
	cargo test

lint:
	cargo fmt --check
	cargo clippy --all-targets
