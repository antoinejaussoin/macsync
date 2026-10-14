# macsync

MacSync is a menu-bar app for macOS, built with SwiftUI, that keeps dated snapshots of folders using
[rsync](https://rsync.samba.org/) from Homebrew. Each run creates a complete,
browsable copy of the source folder; files that did not change are hard-linked
to the previous snapshot, so a snapshot only costs the space of what changed
(the same idea as Time Machine).

```text
/Volumes/Backup/Documents snapshots/
  2026-10-10T020000/      full tree, unchanged files hard-linked
  2026-10-11T020000/
  latest -> 2026-10-11T020000
```

## Features

- Several folder pairs (source and destination), each with its own schedule.
- Full dated snapshots with `rsync --link-dest`; `latest` only moves after a successful run.
- Tiered retention per pair: last N, one per day / week / month for N periods, one per year (forever or N years), with a preview of what would be removed.
- Optional read-only snapshots (files copied without write permission, folders locked).
- Integrity check by checksum, manually or on its own schedule.
- Stats: source size, space used by all snapshots (hard links counted once), snapshot count and range, free space.
- Menu-bar icon with status, per-pair "back up now", and no Dock icon. Optional launch at login.

## Requirements

- macOS 13 or newer.
- A destination formatted as APFS or Mac OS Extended. exFAT and FAT have no hard links and are refused.
- Swift 5.9 or newer (Xcode 15 or newer) to build. The bundle script downloads and compiles rsync 3.5.1, linked only to macOS system libraries, and puts it in the app. Apple's `/usr/bin/rsync` (openrsync) is not used. `swift test` and `swift run` still use Homebrew rsync when you have not built the app.

## Build and install

```sh
scripts/bundle.sh             # builds dist/MacSync.app
scripts/bundle.sh --install   # and copies it to /Applications
make                          # builds it and copies it to mini:/Users/antoine/macsync over SSH
make HOST=other REMOTE_DIR=/path   # another machine or folder
```

Open `/Applications/MacSync.app`. The window opens on first launch; afterwards
use the menu-bar icon (a clock with an arrow) and "Open MacSync…".

### Full Disk Access

To back up Desktop, Documents, Downloads, Photos libraries and other protected
folders, add MacSync in System Settings > Privacy & Security > Full Disk Access
(the welcome screen has a button for this). Without it rsync fails with exit
code 23 ("some files could not be copied").

The bundle is ad-hoc signed by default, so macOS treats each rebuild as a new
app and the grant has to be given again. Set `SIGN_IDENTITY` to a certificate
name from your keychain to keep it across builds:

```sh
SIGN_IDENTITY="Apple Development: you@example.com (TEAMID)" scripts/bundle.sh --install
```

## How it works

### A backup run

1. Checks the folders: the destination must already exist (an unplugged drive never becomes a new folder under `/Volumes`), neither folder may contain the other, and an empty source is refused.
2. Removes any `.inprogress-*` folder left by an interrupted run.
3. Runs, without a shell:

   ```text
   rsync -aHAX --delete --delete-excluded --info=stats2,progress2 \
     [--chmod=Fa-w] [--exclude=PATTERN ...] \
     --link-dest=<previous snapshot> <source>/ <destination>/.inprogress-<stamp>/
   ```

   `-aHAX` keeps permissions, times, symlinks, hard links, ACLs and extended attributes.
4. Copying starts immediately. One rsync command does the backup. On a local disk that command forks into three processes (the client, and the two sides of its internal pipe); that is rsync itself, not three backups. The on-screen time left is rsync's own estimate. Bytes copied, rate, and files transferred come from that same progress line. Source and snapshot sizes are not measured unless you press Measure disk usage.
5. On success (exit 0, or 24 when files vanished mid-copy) renames the folder to its stamp, points `latest` at it, applies retention, then locks the new snapshot's folders if read-only is on. On failure `latest` is not touched.

Only one rsync runs at a time; other jobs wait in a queue.

### Retention

Applied after every successful backup. Defaults:

| Tier | Default |
| --- | --- |
| Last | 30 snapshots |
| Daily | off |
| Weekly | off |
| Monthly | 3 months |
| Yearly | every year, forever |

Each calendar tier keeps the newest snapshot of each of its N most recent
periods. A period already covered by another tier counts, so it is not stored
twice. With daily backups and the defaults you keep the last 30 days, the end
of the month before those, and the last snapshot of every previous year. The
snapshot `latest` points at is never removed, and only folders named like
`2026-10-11T020000` are ever deleted.

### Read-only snapshots

With the option on (the default), files are copied with `--chmod=Fa-w`, so
`--link-dest` still sees matching permissions and keeps hard-linking, and the
snapshot's folders are made read-only after the run. Files restored from a
snapshot come back read-only; `chmod -R u+w` fixes that. Turning the option on
or off makes the next backup copy every file once.

### Integrity check

```text
rsync -aHAX --dry-run --checksum --delete --out-format=%i|%n <source>/ <latest snapshot>/
```

Nothing is written. Every file in the source is compared with the latest
snapshot by content, and the result is grouped into:

- **Same size and date, different content**: the signal for silent corruption or tampering (reported as a failure and a notification).
- Edited, new or deleted since the snapshot: normal changes that the next backup picks up.

This checks the latest snapshot against the live source. Older snapshots are
not compared, since their files legitimately differ from today's source.

### Schedules

Schedules are standard 5-field cron expressions (`minute hour day month
weekday`, Sunday = 0), evaluated by the app itself every 15 seconds. The UI
offers presets (hourly, daily, weekly) and a custom field. If the Mac was asleep
or the destination drive was unplugged at the scheduled time, the job runs once
as soon as possible. Empty means manual only.

### Launch at login

The "Launch at login" switch writes `~/Library/LaunchAgents/com.macsync.agent.plist`,
which starts the app with `--background` at the next login (window hidden,
icon in the menu bar). Turn it on from the installed `/Applications/MacSync.app`
so the login item points at that copy.

## Restoring

Snapshots are plain folders. Copy files back with Finder, or with rsync:

```sh
rsync -aHAX "/Volumes/Backup/Documents snapshots/2026-10-11T020000/Report.pages" ~/Documents/
```

## What this protects against

Ransomware that encrypts or deletes files in the source cannot reach older
snapshots through the source, so you can restore from before the attack. Hard
links mean snapshots share disk blocks: this is version history, not multiple
independent copies, and damage to the destination drive affects them all.
Read-only snapshots stop casual rewrites, but anything running as your user can
undo the permissions. For real isolation, unplug the backup drive between runs
or rotate a second, offline drive.

## Files

| What | Where |
| --- | --- |
| Configuration | `~/Library/Application Support/MacSync/config.toml` |
| Run history and stats | `~/Library/Application Support/MacSync/state.toml` |
| Logs (one per pair) | `~/Library/Logs/MacSync/<pair-id>.log` |
| Login item | `~/Library/LaunchAgents/com.macsync.agent.plist` |

Example `config.toml`:

```toml
[[pair]]
id = "4f1c2a9b7d10"
name = "Documents"
source = "/Users/me/Documents"
destination = "/Volumes/Backup/Documents snapshots"
enabled = true
backup_schedule = "0 * * * *"
integrity_schedule = "0 4 * * 0"
excludes = [".DS_Store", "node_modules/"]
read_only = true

[pair.retention]
keep_last = 30
keep_daily = 0
keep_weekly = 0
keep_monthly = 3
keep_yearly = "forever"   # or a number of years
```

## Development

```sh
swift test                                    # unit tests, including an rsync snapshot test when Homebrew rsync is installed
MACSYNC_HOME=/tmp/macsync-dev swift run       # separate config, state and logs
```
