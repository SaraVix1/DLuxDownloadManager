use clap::{Args, Parser, Subcommand};
use std::net::IpAddr;
use std::path::PathBuf;

/// dlux — Download everything. Beautifully.
///
/// A modern download manager for aria2 with a mouse-driven terminal UI and a web UI.
#[derive(Debug, Parser)]
#[command(name = "dlux", version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Config file to use instead of ~/.config/dlux/config.toml
    #[arg(long, global = true, env = "DLUX_CONFIG", value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// aria2 JSON-RPC URL (overrides the config file)
    #[arg(long, global = true, env = "DLUX_RPC_URL", value_name = "URL")]
    pub rpc_url: Option<String>,

    /// aria2 RPC secret (overrides the config file)
    #[arg(
        long,
        global = true,
        env = "DLUX_RPC_SECRET",
        value_name = "SECRET",
        hide_env_values = true
    )]
    pub rpc_secret: Option<String>,

    /// Never start aria2c automatically; fail if aria2 is not running
    #[arg(long, global = true)]
    pub no_spawn: bool,

    /// Increase log verbosity (-v info, -vv debug, -vvv trace). Logs go to stderr.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open the terminal UI (the default)
    Tui,
    /// Run the web UI server
    Serve(ServeArgs),
    /// Print aria2 status and the download list
    Status {
        /// Print the full snapshot as JSON
        #[arg(long)]
        json: bool,
    },
    /// Inspect or create the config file
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Port to listen on
    #[arg(long, short, default_value_t = 8080)]
    pub port: u16,
    /// Address to bind. Anything other than a loopback address requires a web token.
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: IpAddr,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the config file path
    Path,
    /// Print the effective configuration (secrets redacted)
    Show,
    /// Write a config file with the default settings
    Init {
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_subcommand_means_tui() {
        let cli = Cli::try_parse_from(["dlux"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn serve_defaults_to_localhost_8080() {
        let cli = Cli::try_parse_from(["dlux", "serve"]).unwrap();
        let Some(Command::Serve(args)) = cli.command else {
            panic!("expected serve");
        };
        assert_eq!(args.port, 8080);
        assert!(args.bind.is_loopback());

        let cli =
            Cli::try_parse_from(["dlux", "serve", "--port", "9000", "--bind", "0.0.0.0"]).unwrap();
        let Some(Command::Serve(args)) = cli.command else {
            panic!("expected serve");
        };
        assert_eq!(args.port, 9000);
        assert!(!args.bind.is_loopback());
    }

    #[test]
    fn global_flags_work_after_subcommand() {
        let cli = Cli::try_parse_from([
            "dlux",
            "status",
            "--rpc-url",
            "http://127.0.0.1:6801/jsonrpc",
            "--no-spawn",
        ])
        .unwrap();
        assert_eq!(
            cli.global.rpc_url.as_deref(),
            Some("http://127.0.0.1:6801/jsonrpc")
        );
        assert!(cli.global.no_spawn);
    }
}
