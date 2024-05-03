use std::sync::Arc;

use anyhow::Result;
use ethers::prelude::*;
use ethers::types::GethDebugTracerType;
use futures::try_join;
use reqwest::ClientBuilder;
use tracing::debug;

use super::{async_trait, jerigon::RpcBlockMetadata, ProverInput, RpcClient};

mod block;
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
        let (block_trace, rpc_block_metadata) = try_join!(
            block::process_block_trace(Arc::clone(&self.provider), block_number),
            RpcBlockMetadata::fetch(
                Arc::new(ClientBuilder::new().http1_only().build()?),
                &self.rpc_url,
                block_number,
                checkpoint_block_number,
            )
        )?;

        debug!("Got block result: {:?}", rpc_block_metadata.block_by_number);
        debug!("Got trace result: {:?}", block_trace);
        debug!("Got chain_id: {:?}", rpc_block_metadata.chain_id);

        Ok(ProverInput {
            block_trace,
            other_data: rpc_block_metadata.into(),
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
