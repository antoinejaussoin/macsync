# Deployed next to the macsync binary. On the Linux box:
#   make install-service
#   make uninstall-service

PREFIX := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
UNIT_DIR := $(HOME)/.config/systemd/user
UNIT := $(UNIT_DIR)/macsync.service

.PHONY: install-service uninstall-service status

install-service:
	@test -x "$(PREFIX)/macsync" || (echo "error: missing executable $(PREFIX)/macsync"; exit 1)
	@test -f "$(PREFIX)/config.toml" || (echo "error: missing $(PREFIX)/config.toml"; echo "  cp config.example.toml config.toml && edit it"; exit 1)
	loginctl enable-linger "$(USER)"
	mkdir -p "$(UNIT_DIR)"
	@printf '%s\n' \
		'[Unit]' \
		'Description=MacSync rsync snapshot daemon' \
		'After=network-online.target' \
		'Wants=network-online.target' \
		'' \
		'[Service]' \
		'Type=simple' \
		'ExecStart=$(PREFIX)/macsync' \
		'ExecReload=/bin/kill -HUP $$MAINPID' \
		'Restart=on-failure' \
		'RestartSec=10' \
		'' \
		'[Install]' \
		'WantedBy=default.target' \
		> "$(UNIT)"
	systemctl --user daemon-reload
	systemctl --user enable --now macsync
	@echo "Installed user service (ExecStart=$(PREFIX)/macsync)"
	@systemctl --user --no-pager status macsync || true

uninstall-service:
	-systemctl --user disable --now macsync
	rm -f "$(UNIT)"
	systemctl --user daemon-reload
	@echo "Removed macsync user service."
	@echo "Linger is unchanged; to disable: loginctl disable-linger $(USER)"

status:
	systemctl --user --no-pager status macsync
