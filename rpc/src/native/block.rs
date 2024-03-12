use std::sync::Arc;

use alloy::{
    providers::Provider,
    rpc::types::eth::{BlockId, BlockTransactionsKind},
    transports::Transport,
};
use anyhow::Context as _;
use trace_decoder::trace_protocol::BlockTrace;

/// Processes the block with the given block number and returns the block trace.
pub async fn process_block_trace<ProviderT, TransportT>(
    provider: Arc<ProviderT>,
    block_number: BlockId,
) -> anyhow::Result<BlockTrace>
where
    ProviderT: Provider<TransportT>,
    TransportT: Transport + Clone,
{
    let block = provider
        .get_block(block_number, BlockTransactionsKind::Full)
        .await?
        .context("target block does not exist")?;

    let (code_db, txn_info) = super::txn::process_transactions(&block, provider.clone()).await?;
    let trie_pre_images =
        super::state::process_state_witness(provider.clone(), block, &txn_info).await?;

    Ok(BlockTrace {
        txn_info,
        code_db: Option::from(code_db).filter(|x| !x.is_empty()),
        trie_pre_images,
    })
}
