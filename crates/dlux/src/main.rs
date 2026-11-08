//! `dlux` — Download everything. Beautifully.

mod cli;
mod status;
mod tui;

use anyhow::{Result, bail};
use clap::Parser;
use cli::{Cli, Command, ConfigCommand, GlobalArgs, ServeArgs};
use dlux_core::aria2::{self, Aria2, ConnectOptions};
use dlux_core::{Config, paths};
use std::path::PathBuf;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let verbose = cli.global.verbose;
    // The TUI owns the terminal, so it logs to a file instead of stderr.
    let log_to_file = matches!(cli.command, None | Some(Command::Tui));
    init_tracing(verbose, log_to_file);
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("dlux: {}", error_message(&e, verbose > 0));
            ExitCode::FAILURE
        }
    }
}

/// The top-level error plus its root cause; the whole chain when `verbose`.
fn error_message(e: &anyhow::Error, verbose: bool) -> String {
    if verbose {
        return format!("{e:#}");
    }
    let top = e.to_string();
    let root = e.root_cause().to_string();
    if e.chain().count() > 1 && !top.contains(&root) {
        format!("{top} ({root})")
    } else {
        top
    }
}

/// Log to stderr, or to `~/.local/share/dlux/dlux.log` when `to_file` (falling back to no
/// logging if the file cannot be opened).
fn init_tracing(verbose: u8, to_file: bool) {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_env("DLUX_LOG").unwrap_or_else(|_| EnvFilter::new(default));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false);
    if to_file {
        let dir = paths::data_dir();
        let file = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("dlux.log"))
        });
        if let Ok(file) = file {
            builder
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .init();
        }
    } else {
        builder.with_writer(std::io::stderr).init();
    }
}

fn config_path(global: &GlobalArgs) -> PathBuf {
    global
        .config
        .as_ref()
        .map(paths::expand_tilde)
        .unwrap_or_else(paths::config_file)
}

/// Load the config file and apply command-line overrides.
fn load_config(global: &GlobalArgs) -> Result<Config> {
    let mut cfg = Config::load_from(&config_path(global))?;
    if let Some(url) = &global.rpc_url {
        cfg.aria2.rpc_url.clone_from(url);
    }
    if let Some(secret) = &global.rpc_secret {
        cfg.aria2.rpc_secret = Some(secret.clone()).filter(|s| !s.is_empty());
    }
    if global.no_spawn {
        cfg.aria2.auto_spawn = false;
    }
    Ok(cfg)
}

async fn run(cli: Cli) -> Result<()> {
    let command = cli.command.unwrap_or(Command::Tui);
    match command {
        Command::Config(cmd) => config_command(&cli.global, cmd),
        Command::Serve(args) => serve(&cli.global, &args),
        Command::Tui => {
            with_aria2(&cli.global, async |aria2, cfg| tui::run(aria2, cfg).await).await
        }
        Command::Status { json } => {
            with_aria2(&cli.global, async |aria2, _| {
                status::print(aria2, json).await
            })
            .await
        }
    }
}

/// Connect to (or spawn) aria2, run `f`, and always stop a spawned aria2 afterwards — also on
/// error and on Ctrl-C.
async fn with_aria2(
    global: &GlobalArgs,
    f: impl AsyncFnOnce(&Aria2, &Config) -> Result<()>,
) -> Result<()> {
    let cfg = load_config(global)?;
    let aria2 = aria2::connect(&ConnectOptions::from_config(&cfg)).await?;
    let result = tokio::select! {
        r = f(&aria2, &cfg) => r,
        _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("interrupted")),
    };
    aria2.shutdown().await;
    result
}

fn serve(global: &GlobalArgs, args: &ServeArgs) -> Result<()> {
    let cfg = load_config(global)?;
    if !args.bind.is_loopback() && cfg.web.token.is_none() {
        bail!(
            "refusing to bind to {} without authentication: set `token` under [web] in {}",
            args.bind,
            config_path(global).display()
        );
    }
    bail!(
        "the web UI is not built yet (milestone 6); it will listen on http://{}:{}",
        args.bind,
        args.port
    )
}

fn config_command(global: &GlobalArgs, cmd: ConfigCommand) -> Result<()> {
    let path = config_path(global);
    match cmd {
        ConfigCommand::Path => println!("{}", path.display()),
        ConfigCommand::Show => {
            let mut cfg = load_config(global)?;
            redact(&mut cfg.aria2.rpc_secret);
            redact(&mut cfg.web.token);
            print!("{}", cfg.to_toml());
        }
        ConfigCommand::Init { force } => {
            if path.exists() && !force {
                bail!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                );
            }
            Config::default().save_to(&path)?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}

fn redact(secret: &mut Option<String>) {
    if secret.is_some() {
        *secret = Some("<redacted>".to_owned());
    }
}
