use std::{io, sync::Arc};

<<<<<<< HEAD
use alloy::{providers::RootProvider, rpc::types::eth::BlockId};
use clap::{Parser, ValueHint};
use common::block_interval::BlockInterval;
=======
use clap::{Parser, ValueEnum, ValueHint};
use rpc::retry::build_http_retry_provider;
>>>>>>> 7cd3d62 (Introduce native tracer support)
use tracing_subscriber::{prelude::*, EnvFilter};
use url::Url;

#[derive(Parser)]
pub enum Cli {
    /// Fetch and generate prover input from the RPC endpoint
    Fetch {
<<<<<<< HEAD
        // Starting block of interval to fetch
        #[arg(short, long)]
        start_block: u64,
        // End block of interval to fetch
        #[arg(short, long)]
        end_block: u64,
        /// The RPC URL.
        #[arg(short = 'u', long, value_hint = ValueHint::Url)]
        rpc_url: Url,
        /// The checkpoint block number. If not provided,
        /// block before the `start_block` is the checkpoint
        #[arg(short, long)]
        checkpoint_block_number: Option<BlockId>,
=======
        /// The RPC URL
        #[arg(short = 'u', long, value_hint = ValueHint::Url)]
        rpc_url: Url,
        /// The RPC Tracer Type
        #[arg(short = 't', long, default_value = "jerigon")]
        rpc_type: RpcType,
        /// The block number
        #[arg(short, long)]
        block_number: u64,
        /// The checkpoint block number
        #[arg(short, long, default_value_t = 0)]
        checkpoint_block_number: u64,
        /// Backoff in milliseconds for request retries
        #[arg(long, default_value_t = 0)]
        backoff: u64,
        /// The maximum number of retries
        #[arg(long, default_value_t = 0)]
        max_retries: u32,
>>>>>>> 7cd3d62 (Introduce native tracer support)
    },
}

/// The RPC type.
#[derive(ValueEnum, Clone)]
pub enum RpcType {
    Jerigon,
    Native,
}

impl Cli {
    /// Execute the cli command.
    pub async fn execute(self) -> anyhow::Result<()> {
        match self {
            Self::Fetch {
                rpc_url,
                rpc_type,
                block_number,
                checkpoint_block_number,
                backoff,
                max_retries,
            } => {
                let prover_input = match rpc_type {
                    RpcType::Jerigon => {
                        rpc::jerigon::prover_input(
                            build_http_retry_provider(rpc_url, backoff, max_retries),
                            block_number.into(),
                            checkpoint_block_number.into(),
                        )
                        .await?
                    }
                    RpcType::Native => {
                        rpc::native::prover_input(
                            Arc::new(build_http_retry_provider(rpc_url, backoff, max_retries)),
                            block_number.into(),
                            checkpoint_block_number.into(),
                        )
                        .await?
                    }
                };
                serde_json::to_writer_pretty(io::stdout(), &prover_input)?;
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::Registry::default()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .compact()
                .with_filter(EnvFilter::from_default_env()),
        )
        .init();

<<<<<<< HEAD
    let Args::Fetch {
        start_block,
        end_block,
        rpc_url,
        checkpoint_block_number,
    } = Args::parse();

    let checkpoint_block_number = checkpoint_block_number.unwrap_or((start_block - 1).into());
    let block_interval = BlockInterval::Range(start_block..end_block + 1);

    // Retrieve prover input from the Erigon node
    let prover_input = rpc::prover_input(
        RootProvider::new_http(rpc_url),
        block_interval,
        checkpoint_block_number,
    )
    .await?;

    serde_json::to_writer_pretty(io::stdout(), &prover_input.blocks)?;

=======
    Cli::parse().execute().await?;
>>>>>>> 7cd3d62 (Introduce native tracer support)
    Ok(())
}
