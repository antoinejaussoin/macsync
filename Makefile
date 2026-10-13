HOST ?= mini
REMOTE_DIR ?= /Users/antoine/macsync
APP := dist/MacSync.app

.PHONY: deploy bundle install run test lint

deploy: bundle
	ssh $(HOST) 'mkdir -p "$(REMOTE_DIR)"'
	rsync -a --delete -e ssh $(APP) $(HOST):"$(REMOTE_DIR)/"
	@echo "Copied MacSync.app to $(HOST):$(REMOTE_DIR)"

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
