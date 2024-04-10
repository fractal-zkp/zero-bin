#![allow(clippy::needless_range_loop)]
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use ethers::prelude::*;
use ethers::types::GethDebugTracerType;
use futures::stream::{self, TryStreamExt};
use reqwest::Client;
use trace_decoder::trace_protocol::{BlockTrace, TxnInfo};

use super::{async_trait, jerigon::RpcBlockMetadata, ProverInput, RpcClient};

mod state;
mod trie;
mod txn;

/// NATIVE RPC CLIENT
/// ===============================================================================================

/// The native RPC client.
pub struct NativeRpcClient {
    provider: Provider<Http>,
    rpc_url: String,
}

impl NativeRpcClient {
    /// Creates a new `NativeRpcClient` with the given RPC URL.
    pub fn new(rpc_url: String) -> Result<Self> {
        let provider = Provider::<Http>::try_from(rpc_url.clone())?;
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
        let accounts_state = Arc::new(Mutex::new(BTreeMap::<H160, AccountState>::new()));
        let block = self
            .provider
            .get_block(block_number)
            .await?
            .ok_or_else(|| anyhow!("Block not found. Block number: {}", block_number))?;

        let tx_infos = stream::iter(&block.transactions)
            .then(|tx_hash| {
                let arc = accounts_state.clone();
                async move { txn::process_transaction(&self.provider, tx_hash, arc).await }
            })
            .try_collect::<Vec<TxnInfo>>()
            .await?;

        let block_trace = BlockTrace {
            txn_info: tx_infos,
            trie_pre_images: state::process_state_witness(&self.provider, block, accounts_state)
                .await?,
        };

        Ok(ProverInput {
            block_trace,
            other_data: RpcBlockMetadata::fetch(
                &Client::new(),
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
