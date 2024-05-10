use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use ethers::prelude::*;
use ethers::providers::{Http, Provider};
use ethers::types::{H160, H256};
use futures::stream::{self, TryStreamExt};
use tokio::sync::Mutex;
use trace_decoder::trace_protocol::{AtomicUnitInfo, BlockTrace, TriePreImage, TxnInfo};

/// Processes the block with the given block number and returns the block trace.
pub async fn process_block_trace(
    provider: Arc<Provider<Http>>,
    block_number: u64,
) -> Result<BlockTrace> {
    let block = provider
        .get_block(block_number)
        .await?
        .ok_or_else(|| anyhow!("Block not found. Block number: {}", block_number))?;

    let mut accounts_state = HashMap::<H160, HashSet<H256>>::new();

    block.withdrawals.as_ref().map(|w| {
        w.iter().for_each(|w| {
            accounts_state.insert(w.address, Default::default());
        })
    });
    accounts_state.insert(
        block
            .author
            .ok_or_else(|| anyhow!("Block author not found"))?,
        Default::default(),
    );

    let accounts_state = Arc::new(Mutex::new(accounts_state));
    let code_db = Arc::new(Mutex::new(HashMap::<H256, Vec<u8>>::new()));

    let tx_infos = stream::iter(&block.transactions)
        .then(|tx_hash| {
            let accounts_state = accounts_state.clone();
            let provider = Arc::clone(&provider);
            let code_db = Arc::clone(&code_db);
            async move {
                super::txn::process_transaction(provider, tx_hash, accounts_state, code_db).await
            }
        })
        .try_collect::<Vec<TxnInfo>>()
        .await?;

    let trie_pre_images =
        super::state::process_state_witness(Arc::clone(&provider), block, accounts_state).await?;

    Ok(BlockTrace {
        atomic_info: AtomicUnitInfo::Txn(tx_infos),
        code_db: Some(
            Arc::try_unwrap(code_db)
                .map_err(|_| anyhow!("Lock still has multiple owners"))?
                .into_inner(),
        ),
        trie_pre_images: TriePreImage::Mpt(trie_pre_images),
    })
}
