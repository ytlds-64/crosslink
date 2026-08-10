mod input;
mod net;
mod switch;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};

use net::crypto;
use net::transport;
use switch::Side;

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

    /// Disable keyboard+mouse capture on the server (server still accepts Input
    /// messages from peer, but won't send its own local input).
    #[arg(long)]
    no_capture: bool,

    /// Disable keyboard+mouse injection on the client (client still forwards
    /// received Input events, but won't call SendInput locally).
    #[arg(long)]
    no_inject: bool,

    /// Server: send 5 mock key events 500ms after handshake (for end-to-end
    /// pipeline testing in sandbox/CI without real keypress).
    #[arg(long)]
    test_input: bool,

    /// Enable edge-switching (Universal Control style): the pointer roams between
    /// this machine and the peer; only the machine currently under the pointer is
    /// controlled. Requires both ends to run --switch with consistent --side.
    #[arg(long)]
    switch: bool,

    /// In --switch mode, where the peer machine sits relative to this one.
    /// Defaults: server = right, client = left (a coherent horizontal layout).
    /// Override on both ends consistently for other layouts.
    #[arg(long, value_enum)]
    side: Option<SideArg>,

    /// Enable M4 seamless single-cursor mode: the Windows server drives one logical
    /// cursor that slides from the Win screen onto the Mac screen. Requires the Win
    /// machine to be the server and the Mac to be the client. Mutually exclusive with
    /// --switch (edge-switching). In M4 the Win mouse keeps controlling the Mac cursor
    /// continuously, instead of just handing off ownership.
    #[arg(long)]
    m4: bool,
}

/// CLI 侧 `--side` 取值（clap ValueEnum），运行时映射为 `switch::Side`。
#[derive(ValueEnum, Clone, Copy, Debug)]
enum SideArg {
    Right,
    Left,
    Top,
    Bottom,
}

impl From<SideArg> for Side {
    fn from(s: SideArg) -> Self {
        match s {
            SideArg::Right => Side::Right,
            SideArg::Left => Side::Left,
            SideArg::Top => Side::Top,
            SideArg::Bottom => Side::Bottom,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();

    if cli.m4 && cli.switch {
        eprintln!("Error: --m4 (seamless single-cursor) and --switch (edge-switching) are mutually exclusive");
        std::process::exit(2);
    }

    // --switch 模式下，对端相对本机的位置：服务端默认右、客户端默认左。
    let side: Side = cli
        .side
        .map(Side::from)
        .unwrap_or(if cli.server { Side::Right } else { Side::Left });

    match (cli.server, cli.client.clone()) {
        (true, _) => {
            let path = PathBuf::from("crosslink-server.key");
            let key = crypto::load_or_create_server_key(&path)?;
            let (_pk, fp) = crypto::public_key_and_fingerprint(&key);
            log::info!("=== Server identity fingerprint (share this with clients) ===");
            log::info!("    {}", fp);
            log::info!("===========================================================");
            transport::run_server(
                &cli.bind,
                cli.port,
                &key,
                &cli.name,
                !cli.no_capture,
                cli.test_input,
                cli.switch,
                side,
                cli.m4,
            )
            .await?;
        }
        (false, Some(addr)) => {
            transport::run_client(
                &addr,
                cli.port,
                cli.fingerprint.as_deref(),
                &cli.name,
                !cli.no_inject,
                cli.switch,
                side,
                cli.m4,
            )
            .await?;
        }
        (false, None) => {
            eprintln!("Usage:");
            eprintln!("  crosslink --server [--no-capture] [--switch [--side right|left|top|bottom]]");
            eprintln!("  crosslink --client <ADDR> [--fingerprint <FP>] [--port 4242] [--no-inject] [--switch [--side ...]]");
            std::process::exit(2);
        }
    }

    Ok(())
}
