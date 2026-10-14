use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
pub struct Cli {
    #[arg(short, long, value_name = "FILE")]
    pub config: PathBuf,

    #[arg(short, long)]
    pub verbose: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_defaults_off() {
        let cli = Cli::try_parse_from(["sama", "--config", "config.toml"]).unwrap();
        assert!(!cli.verbose);
    }

    #[test]
    fn verbose_flag_enables_per_connection_logs() {
        for flag in ["--verbose", "-v"] {
            let cli = Cli::try_parse_from(["sama", "--config", "config.toml", flag]).unwrap();
            assert!(cli.verbose, "flag {flag} should set verbose");
        }
    }
}
