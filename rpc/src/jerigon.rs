use alloy::{
    primitives::B256,
    providers::Provider,
    rpc::types::eth::{BlockId, BlockNumberOrTag, BlockTransactionsKind},
    transports::Transport,
};
use anyhow::Context as _;
use common::block_interval::BlockInterval;
use futures::StreamExt as _;
use prover::BlockProverInput;
use prover::ProverInput;
use serde::Deserialize;
use serde_json::json;
use trace_decoder::trace_protocol::{
    BlockTrace, BlockTraceTriePreImages, CombinedPreImages, TrieCompact, TxnInfo,
};

use super::fetch_other_block_data;

/// Transaction traces retrieved from Erigon zeroTracer.
#[derive(Debug, Deserialize)]
pub struct ZeroTxResult {
    #[serde(rename(deserialize = "txHash"))]
    pub tx_hash: alloy::primitives::TxHash,
    pub result: TxnInfo,
}

/// Block witness retrieved from Erigon zeroTracer.
#[derive(Debug, Deserialize)]
pub struct ZeroBlockWitness(TrieCompact);

pub async fn block_prover_input<ProviderT, TransportT>(
    provider: ProviderT,
    target_block_id: BlockId,
    checkpoint_state_trie_root: B256,
) -> anyhow::Result<BlockProverInput>
where
    ProviderT: Provider<TransportT>,
    TransportT: Transport + Clone,
{
    // Grab trace information
    let tx_results = provider
        .raw_request::<_, Vec<ZeroTxResult>>(
            "debug_traceBlockByNumber".into(),
            (target_block_id, json!({"tracer": "zeroTracer"})),
        )
        .await?;

    // Grab block witness info (packed as combined trie pre-images)
    let block_witness = provider
        .raw_request::<_, ZeroBlockWitness>("eth_getWitness".into(), vec![target_block_id])
        .await?;

    let other_data =
        fetch_other_block_data(provider, target_block_id, checkpoint_state_trie_root).await?;

    // Assemble
    Ok(BlockProverInput {
        block_trace: BlockTrace {
            trie_pre_images: BlockTraceTriePreImages::Combined(CombinedPreImages {
                compact: block_witness.0,
            }),
            txn_info: tx_results.into_iter().map(|it| it.result).collect(),
            code_db: Default::default(),
        },
        other_data,
    })
}

/// Obtain the prover input for a given block interval
pub async fn prover_input<ProviderT, TransportT>(
    provider: ProviderT,
    block_interval: BlockInterval,
    checkpoint_block_id: BlockId,
) -> anyhow::Result<ProverInput>
where
    ProviderT: Provider<TransportT>,
    TransportT: Transport + Clone,
{
    // Grab interval checkpoint block state trie
    let checkpoint_state_trie_root = provider
        .get_block(checkpoint_block_id, BlockTransactionsKind::Hashes)
        .await?
        .context("block does not exist")?
        .header
        .state_root;

    let mut block_proofs = Vec::new();
    let mut block_interval = block_interval.into_bounded_stream()?;

    while let Some(block_num) = block_interval.next().await {
        let block_id = BlockId::Number(BlockNumberOrTag::Number(block_num));
        let block_prover_input =
            block_prover_input(&provider, block_id, checkpoint_state_trie_root).await?;
        block_proofs.push(block_prover_input);
    }
    Ok(ProverInput {
        blocks: block_proofs,
    })
}
