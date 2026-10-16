HOST ?= mini
REMOTE_DIR ?= /Users/antoine/macsync
APP := dist/MacSync.app

LINUX_HOST ?= 192.168.0.251
LINUX_REMOTE_DIR ?= /home/antoine
LINUX_BIN := linux/target/x86_64-unknown-linux-gnu/release/macsync

.PHONY: deploy deploy-linux bundle install run test lint linux

deploy: bundle
	ssh $(HOST) 'mkdir -p "$(REMOTE_DIR)"'
	rsync -a --delete -e ssh $(APP) $(HOST):"$(REMOTE_DIR)/"
	@echo "Copied MacSync.app to $(HOST):$(REMOTE_DIR)"

deploy-linux: linux
	rsync -a -e ssh "$(LINUX_BIN)" $(LINUX_HOST):"$(LINUX_REMOTE_DIR)/macsync"
	@echo "Copied macsync to $(LINUX_HOST):$(LINUX_REMOTE_DIR)/macsync"

bundle:
	scripts/bundle.sh

install:
	scripts/bundle.sh --install

run: bundle
	-pkill -x MacSync
	@sleep 0.4
	open "$(APP)"

test:
	swift test

lint:
	swift build --build-tests

linux:
	$(MAKE) -C linux linux-amd64
