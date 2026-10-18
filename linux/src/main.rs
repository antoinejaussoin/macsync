mod backup;
mod config;
mod engine;
mod integrity;
mod logutil;
mod notify;
mod paths;
mod retention;
mod rsync;
mod schedule;
mod state;

use config::EXAMPLE_CONFIG;
use paths::{config_path, data_dir, state_path};

fn usage() -> String {
    format!(
        "\
macsync — Linux daemon for dated, hard-linked rsync snapshots over SSH

Usage:
  macsync                 Run in the foreground (systemd Type=simple)
  macsync --help          Print this help and a config example
  macsync --interactive   One-shot: pick a pair and back it up now
  macsync --test-notify   Send a test ntfy.sh notification and exit

Any other argument prints this text on stderr and exits with status 2.

Paths (same directory as the binary unless overridden):
  config  {config}  (MACSYNC_CONFIG overrides)
  state   {state}   (MACSYNC_HOME overrides the data dir)
  logs    {data}/logs/

Example config.toml:

{example}
",
        config = config_path().display(),
        state = state_path().display(),
        data = data_dir().display(),
        example = EXAMPLE_CONFIG.trim_end()
    )
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        None => {
            if let Err(e) = engine::run_daemon() {
                eprintln!("macsync: {e}");
                std::process::exit(1);
            }
        }
        Some("--help") | Some("-h") => {
            print!("{}", usage());
            std::process::exit(0);
        }
        Some("--interactive") => {
            if args.next().is_some() {
                eprint!("{}", usage());
                std::process::exit(2);
            }
            if let Err(e) = engine::run_interactive() {
                eprintln!("macsync: {e}");
                std::process::exit(1);
            }
        }
        Some("--test-notify") => {
            if args.next().is_some() {
                eprint!("{}", usage());
                std::process::exit(2);
            }
            if let Err(e) = engine::run_test_notify() {
                eprintln!("macsync: {e}");
                std::process::exit(1);
            }
        }
        Some(_) => {
            eprint!("{}", usage());
            std::process::exit(2);
        }
    }
}
