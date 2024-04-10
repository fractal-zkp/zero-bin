use std::io::Write;

use anyhow::Result;
use clap::Parser;
use cli::Commands;
use rpc::{JerigonRpcClient, NativeRpcClient, RpcClient};

mod cli;
mod init;
mod rpc;

#[tokio::main]
async fn main() -> Result<()> {
    init::tracing();
    let args = cli::Cli::parse();

    match args.command {
        Commands::Fetch {
            rpc_url,
            rpc_type,
            block_number,
            checkpoint_block_number,
        } => {
            let prover_input = match rpc_type.as_str() {
                "jerigon" => {
                    let client = JerigonRpcClient::new(rpc_url);
                    client
                        .fetch_prover_input(block_number, checkpoint_block_number)
                        .await?
                }
                "native" => {
                    let client = NativeRpcClient::new(rpc_url)
                        .expect("should be able to create native rpc client");
                    client
                        .fetch_prover_input(block_number, checkpoint_block_number)
                        .await?
                }
                _ => panic!("Invalid RPC type"),
            };
            std::io::stdout().write_all(&serde_json::to_vec(&prover_input)?)?;
        }
    }
    Ok(())
}
