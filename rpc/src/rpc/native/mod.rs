use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use ethers::prelude::*;
use ethers::types::{GethDebugTracerType, H160, H256};
use futures::stream::{self, TryStreamExt};
use reqwest::ClientBuilder;
use tokio::sync::Mutex;
use trace_decoder::trace_protocol::{BlockTrace, TxnInfo};

use super::{async_trait, jerigon::RpcBlockMetadata, ProverInput, RpcClient};

mod state;
mod trie;
mod txn;

/// NATIVE RPC CLIENT
/// ===============================================================================================

/// The native RPC client.
pub struct NativeRpcClient {
    provider: Arc<Provider<Http>>,
    rpc_url: String,
}

impl NativeRpcClient {
    /// Creates a new `NativeRpcClient` with the given RPC URL.
    pub fn new(rpc_url: String) -> Result<Self> {
        let provider = Arc::new(Provider::<Http>::try_from(rpc_url.clone())?);
        Ok(Self { provider, rpc_url })
    }
}

#[async_trait]
impl RpcClient for NativeRpcClient {
    async fn fetch_prover_input(
        &self,
        block_number: u64,
        checkpoint_block_number: u64,
    ) -> Result<ProverInput> {
        let block = self
            .provider
            .get_block(block_number)
            .await?
            .ok_or_else(|| anyhow!("Block not found. Block number: {}", block_number))?;

        let accounts_state = Arc::new(Mutex::new(HashMap::<H160, HashSet<H256>>::new()));
        let code_db = Arc::new(Mutex::new(HashMap::<H256, Vec<u8>>::new()));
        let tx_infos =
            stream::iter(&block.transactions)
                .then(|tx_hash| {
                    let accounts_state = accounts_state.clone();
                    let provider = Arc::clone(&self.provider);
                    let code_db = Arc::clone(&code_db);
                    async move {
                        txn::process_transaction(provider, tx_hash, accounts_state, code_db).await
                    }
                })
                .try_collect::<Vec<TxnInfo>>()
                .await?;

        let trie_pre_images =
            state::process_state_witness(Arc::clone(&self.provider), block, accounts_state).await?;

        let block_trace = BlockTrace {
            txn_info: tx_infos,
            code_db: Some(
                Arc::try_unwrap(code_db)
                    .map_err(|_| anyhow!("Lock still has multiple owners"))?
                    .into_inner(),
            ),
            trie_pre_images: trie_pre_images,
        };

        Ok(ProverInput {
            block_trace,
            other_data: RpcBlockMetadata::fetch(
                Arc::new(ClientBuilder::new().http1_only().build()?),
                &self.rpc_url,
                block_number,
                checkpoint_block_number,
            )
            .await?
            .into(),
        })
    }
}

/// TRACING OPTIONS
/// ===============================================================================================

/// Tracing options for the debug_traceTransaction call.
fn tracing_options() -> GethDebugTracingOptions {
    GethDebugTracingOptions {
        tracer: Some(GethDebugTracerType::BuiltInTracer(
            GethDebugBuiltInTracerType::PreStateTracer,
        )),

        ..GethDebugTracingOptions::default()
    }
}

fn tracing_options_diff() -> GethDebugTracingOptions {
    GethDebugTracingOptions {
        tracer: Some(GethDebugTracerType::BuiltInTracer(
            GethDebugBuiltInTracerType::PreStateTracer,
        )),

        tracer_config: Some(GethDebugTracerConfig::BuiltInTracer(
            GethDebugBuiltInTracerConfig::PreStateTracer(PreStateConfig {
                diff_mode: Some(true),
            }),
        )),
        ..GethDebugTracingOptions::default()
    }
}
