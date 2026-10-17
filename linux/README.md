# MacSync Linux daemon (Ubuntu 26.04)

Foreground daemon that pulls remote trees over SSH into **local** dated snapshots with hard links (`--link-dest`), the same retention and cron schedules as the Mac app. It runs as **your** user via a systemd **user** service — there is no dedicated `macsync` system account.

## Requirements

On this machine:

- Ubuntu 26.04 (or similar systemd)
- `rsync` **3.1 or newer** (GNU rsync, not openrsync)
- `openssh-client`

```bash
sudo apt update
sudo apt install -y rsync openssh-client
rsync --version   # confirm 3.1+
```

On each remote host: **GNU rsync 3.1+** (not Apple’s openrsync) and an SSH login that can **read** the source folders. On a Mac remote:

```bash
brew install rsync
# Apple Silicon:
ls /opt/homebrew/bin/rsync
# Intel:
ls /usr/local/bin/rsync
```

Then set in `config.toml`:

```toml
remote_rsync_path = "/opt/homebrew/bin/rsync"
```

Without that, macsync’s `-aHAX` fails on the remote with `invalid option -- A`.

## Install the binary

Copy the `macsync` binary (built for `x86_64-unknown-linux-gnu`) to your account:

```bash
mkdir -p ~/.local/bin
install -m 755 macsync ~/.local/bin/macsync
# Ensure ~/.local/bin is on PATH for login shells, e.g. in ~/.profile:
# export PATH="$HOME/.local/bin:$PATH"
```

Build from the repo:

```bash
cd linux
rustup target add x86_64-unknown-linux-gnu
make linux-amd64
# binary: target/x86_64-unknown-linux-gnu/release/macsync
```

On Ubuntu amd64 that uses the host linker. From macOS, `make linux-amd64` uses `zig` as the cross linker (`brew install zig`); the Makefile sets `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER` to `scripts/zig-cc`.

## SSH (your key)

macsync runs `ssh -o BatchMode=yes` by default so **`~/.ssh/config` Host aliases work** (same as `ssh mini`). It will not prompt for a new host key.

1. Prefer a Host entry for the backup machine (`Host mini`, `HostName`, `IdentityFile`, …).
2. Use that same name in `source`, e.g. `antoine@mini:/path`.
3. Confirm non-interactive SSH (what the daemon uses):

```bash
ssh -o BatchMode=yes antoine@mini 'echo ok'
```

If that fails with “Host key verification failed”, add the host once:

```bash
ssh-keyscan mini >> ~/.ssh/known_hosts
```

Only set `[ssh] identity_file` / `port` in `config.toml` when you need to **override** ssh_config.

## Config

`config.toml` lives **next to the binary** (same directory). `MACSYNC_CONFIG` overrides if set.

```bash
# If the binary is ~/macsync:
cp config.example.toml ~/config.toml
$EDITOR ~/config.toml

# If the binary is ~/.local/bin/macsync:
cp config.example.toml ~/.local/bin/config.toml
```

Defaults (all next to the binary; `MACSYNC_CONFIG` / `MACSYNC_HOME` override):

| Path | Purpose |
|------|---------|
| `<dir-of-macsync>/config.toml` | Config |
| `<dir-of-macsync>/state.toml` | Last run / scheduler anchor |
| `<dir-of-macsync>/logs/<id>.log` | Per-pair logs |

Each `[[pair]]` needs:

- `source` = `user@host:absolute/path` (SSH remote)
- `destination` = **absolute local** directory you can write (snapshot root). Create it first:

```bash
mkdir -p /home/me/backups/photos
```

Empty `backup_schedule` / `integrity_schedule` means that job is off (interactive still works).

## Enable linger and the user service

`make deploy-linux` copies `macsync`, a `Makefile`, and `config.example.toml` into `/home/antoine/macsync/` (override with `LINUX_REMOTE_DIR`). On the Linux box:

```bash
cd ~/macsync
cp -n config.example.toml config.toml   # once, then edit
make install-service                    # linger + systemd --user unit + enable --now
make uninstall-service
make status
```

`install-service` points `ExecStart` at the `macsync` next to that Makefile (config/state/logs stay in the same directory).

## Operate

```bash
systemctl --user status macsync
journalctl --user -u macsync -f
systemctl --user reload macsync    # SIGHUP — reread config
systemctl --user stop macsync      # stops the daemon (and the current rsync)
```

Per-pair detail: `logs/<id>.log` next to the binary.

One-off backup (does **not** change the unit):

```bash
macsync --interactive
```

Help and a full config example:

```bash
macsync --help
```

## Notes

- Only one rsync runs at a time (pairs are sequential).
- Version is checked once at startup.
- Destination must be local; hard links and retention live on this disk.
- Snapshot names look like `2026-10-11T081300`; `latest` is a relative symlink updated only after a successful run.
