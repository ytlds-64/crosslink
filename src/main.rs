mod input;
mod net;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use net::crypto;
use net::transport;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Cross-platform keyboard/mouse sharing (software KVM) for Windows + macOS",
    long_about = None
)]
struct Cli {
    /// Run as server (the machine that physically owns the keyboard/mouse)
    #[arg(long)]
    server: bool,

    /// Run as client, connecting to the server at ADDR
    #[arg(long, value_name = "ADDR")]
    client: Option<String>,

    /// TCP port
    #[arg(long, default_value_t = 4242)]
    port: u16,

    /// Expected server identity fingerprint (SHA-256, colon-hex).
    /// If omitted, trust-on-first-use (TOFU) is applied.
    #[arg(long)]
    fingerprint: Option<String>,

    /// Node display name
    #[arg(long, default_value = "node")]
    name: String,

    /// Bind address for the server
    #[arg(long, default_value = "0.0.0.0")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();

    match (cli.server, cli.client.clone()) {
        (true, _) => {
            let path = PathBuf::from("crosslink-server.key");
            let key = crypto::load_or_create_server_key(&path)?;
            let (_pk, fp) = crypto::public_key_and_fingerprint(&key);
            log::info!("=== Server identity fingerprint (share this with clients) ===");
            log::info!("    {}", fp);
            log::info!("===========================================================");
            transport::run_server(&cli.bind, cli.port, &key, &cli.name).await?;
        }
        (false, Some(addr)) => {
            transport::run_client(&addr, cli.port, cli.fingerprint.as_deref(), &cli.name).await?;
        }
        (false, None) => {
            eprintln!("Usage:");
            eprintln!("  crosslink --server");
            eprintln!("  crosslink --client <ADDR> [--fingerprint <FP>] [--port 4242]");
            std::process::exit(2);
        }
    }

    Ok(())
}
