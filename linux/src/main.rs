mod backup;
mod config;
mod engine;
mod integrity;
mod logutil;
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

Any other argument prints this text on stderr and exits with status 2.

Paths (override config with MACSYNC_CONFIG):
  config  {config}
  state   {state}
  data    {data}

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
        Some(_) => {
            eprint!("{}", usage());
            std::process::exit(2);
        }
    }
}
