HOST ?= mini
REMOTE_DIR ?= /Users/antoine/macsync
APP := dist/MacSync.app

LINUX_HOST ?= 192.168.0.251
# Directory on the remote that holds the binary, Makefile, config, state, logs.
LINUX_REMOTE_DIR ?= /home/antoine/macsync
LINUX_BIN := linux/target/x86_64-unknown-linux-gnu/release/macsync
LINUX_DEPLOY_MAKEFILE := linux/deploy/Makefile
LINUX_CONFIG_EXAMPLE := linux/config.example.toml

.PHONY: deploy deploy-linux bundle install run test lint linux

deploy: bundle
	ssh $(HOST) 'mkdir -p "$(REMOTE_DIR)"'
	rsync -a --delete -e ssh $(APP) $(HOST):"$(REMOTE_DIR)/"
	@echo "Copied MacSync.app to $(HOST):$(REMOTE_DIR)"

deploy-linux: linux
	ssh $(LINUX_HOST) 'mkdir -p "$(LINUX_REMOTE_DIR)"'
	rsync -a -e ssh \
		"$(LINUX_BIN)" \
		"$(LINUX_DEPLOY_MAKEFILE)" \
		"$(LINUX_CONFIG_EXAMPLE)" \
		$(LINUX_HOST):"$(LINUX_REMOTE_DIR)/"
	@echo "Deployed to $(LINUX_HOST):$(LINUX_REMOTE_DIR)/"
	@echo "  macsync, Makefile, config.example.toml"
	@echo "On the box: cd $(LINUX_REMOTE_DIR)"
	@echo "            cp -n config.example.toml config.toml  # once"
	@echo "            make install-service"

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
