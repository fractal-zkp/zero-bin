use alloy::{providers::Provider, rpc::types::eth::BlockId, transports::Transport};
use anyhow::Context as _;
use itertools::{Either, Itertools as _};
use prover::ProverInput;
use serde::Deserialize;
use serde_json::json;
use trace_decoder::trace_protocol::{BlockTrace, BlockTraceTriePreImages, TxnInfo};

use super::fetch_other_block_data;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
enum ZeroTrace {
    Result(TxnInfo),
    BlockWitness(BlockTraceTriePreImages),
}

/// Fetches the prover input for the given BlockId.
pub async fn prover_input<ProviderT, TransportT>(
    provider: ProviderT,
    target_block_id: BlockId,
    checkpoint_block_id: BlockId,
) -> anyhow::Result<ProverInput>
where
    ProviderT: Provider<TransportT>,
    TransportT: Transport + Clone,
{
    // Grab trace information
    /////////////////////////
    let traces = provider
        .raw_request::<_, Vec<ZeroTrace>>(
            "debug_traceBlockByNumber".into(),
            (target_block_id, json!({"tracer": "zeroTracer"})),
        )
        .await?;

    let (txn_info, mut pre_images) =
        traces
            .into_iter()
            .partition_map::<Vec<_>, Vec<_>, _, _, _>(|it| match it {
                ZeroTrace::Result(it) => Either::Left(it),
                ZeroTrace::BlockWitness(it) => Either::Right(it),
            });

    let other_data = fetch_other_block_data(provider, target_block_id, checkpoint_block_id).await?;

    // Assemble
    ///////////
    Ok(ProverInput {
        block_trace: BlockTrace {
            trie_pre_images: pre_images.pop().context("trace had no BlockWitness")?,
            code_db: None,
            txn_info,
        },
        other_data,
    })
}
