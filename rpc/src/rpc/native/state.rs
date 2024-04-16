use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Block, H160, H256};
use ethers::utils::keccak256;
use mpt_trie::partial_trie::HashedPartialTrie;
use tokio::sync::Mutex;
use trace_decoder::trace_protocol::{
    BlockTraceTriePreImages, SeparateStorageTriesPreImage, SeparateTriePreImage,
    SeparateTriePreImages, TrieDirect,
};
use trace_decoder::types::HashedStorageAddr;

use super::trie::PartialTrieBuilder;

/// Processes the state witness for the given block.
pub(super) async fn process_state_witness(
    provider: Arc<Provider<Http>>,
    block: Block<H256>,
    accounts_state: Arc<Mutex<HashMap<H160, HashSet<H256>>>>,
) -> Result<BlockTraceTriePreImages> {
    let accounts_state = Arc::try_unwrap(accounts_state)
        .map_err(|e| anyhow!("Failed to unwrap accounts state from arc: {e:?}"))?
        .into_inner();

    let block_number = block
        .number
        .ok_or_else(|| anyhow!("Block number not returned with block"))?;
    let prev_block = provider.get_block(block_number - 1).await?.ok_or_else(|| {
        anyhow!(
            "Previous block not found. Block number: {}",
            block_number - 1
        )
    })?;

    let (state, storage_proofs) = generate_state_witness(
        prev_block.state_root,
        accounts_state,
        provider,
        block_number,
        block,
    )
    .await?;

    Ok(BlockTraceTriePreImages::Separate(SeparateTriePreImages {
        state: SeparateTriePreImage::Direct(TrieDirect(state.build())),
        storage: SeparateStorageTriesPreImage::MultipleTries(
            storage_proofs
                .into_iter()
                .map(|(a, m)| (a, SeparateTriePreImage::Direct(TrieDirect(m.build()))))
                .collect(),
        ),
    }))
}

/// Generates the state witness for the given block.
async fn generate_state_witness(
    prev_state_root: H256,
    accounts_state: HashMap<H160, HashSet<H256>>,
    provider: Arc<Provider<Http>>,
    block_number: ethereum_types::U64,
    block: Block<H256>,
) -> Result<
    (
        PartialTrieBuilder<HashedPartialTrie>,
        HashMap<H256, PartialTrieBuilder<HashedPartialTrie>>,
    ),
    anyhow::Error,
> {
    let mut state = PartialTrieBuilder::new(prev_state_root, Default::default());
    let mut storage_proofs =
        HashMap::<HashedStorageAddr, PartialTrieBuilder<HashedPartialTrie>>::new();

    // Process transaction state accesses
    for (address, keys) in accounts_state.iter() {
        let proof = provider
            .get_proof(
                *address,
                keys.iter().copied().collect(),
                Some((block_number - 1).into()),
            )
            .await?;
        state.insert_proof(proof.account_proof);

        if keys.len() > 0 {
            let mut storage_mpt = PartialTrieBuilder::new(proof.storage_hash, Default::default());
            for proof in proof.storage_proof {
                storage_mpt.insert_proof(proof.proof);
            }

            storage_proofs.insert(keccak256(address).into(), storage_mpt);
        }
    }

    // Process author account access
    let proof = provider
        .get_proof(
            block
                .author
                .ok_or_else(|| anyhow!("Block author not found"))?,
            vec![],
            block.number.map(Into::into),
        )
        .await?;
    state.insert_proof(proof.account_proof);

    // Process withdrawals account access
    if let Some(withdrawals) = block.withdrawals.as_ref() {
        for withdrawal in withdrawals {
            let proof = provider
                // TODO: should this be for the  next block?
                .get_proof(withdrawal.address, vec![], Some((block_number - 1).into()))
                .await?;
            state.insert_proof(proof.account_proof);
        }
    }

    Ok((state, storage_proofs))
}
