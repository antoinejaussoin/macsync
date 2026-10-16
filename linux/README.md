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

```bash
mkdir -p ~/.config/macsync
cp config.example.toml ~/.config/macsync/config.toml
$EDITOR ~/.config/macsync/config.toml
```

Defaults:

| Path | Purpose |
|------|---------|
| `~/.config/macsync/config.toml` | Config (`MACSYNC_CONFIG` overrides) |
| `~/.local/share/macsync/state.toml` | Last run / scheduler anchor |
| `~/.local/share/macsync/logs/<id>.log` | Per-pair logs |

Each `[[pair]]` needs:

- `source` = `user@host:absolute/path` (SSH remote)
- `destination` = **absolute local** directory you can write (snapshot root). Create it first:

```bash
mkdir -p /home/me/backups/photos
```

Empty `backup_schedule` / `integrity_schedule` means that job is off (interactive still works).

## Enable linger and the user service

User services stop at logout unless lingering is on. Enable linger so backups continue after logout and start at boot:

```bash
loginctl enable-linger "$USER"
```

Install the unit:

```bash
mkdir -p ~/.config/systemd/user
cp macsync.service ~/.config/systemd/user/macsync.service
systemctl --user daemon-reload
systemctl --user enable --now macsync
```

The unit is `Type=simple`, `ExecStart=%h/.local/bin/macsync`, `Restart=on-failure`. There is no `User=` line — a user unit already runs as you.

## Operate

```bash
systemctl --user status macsync
journalctl --user -u macsync -f
systemctl --user reload macsync    # SIGHUP — reread config
systemctl --user stop macsync      # stops the daemon (and the current rsync)
```

Per-pair detail: `~/.local/share/macsync/logs/<id>.log`.

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
