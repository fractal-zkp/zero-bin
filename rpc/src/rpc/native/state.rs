use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Block, EIP1186ProofResponse, H160, H256};
use ethers::utils::keccak256;
use futures::stream::{self, StreamExt, TryStreamExt};
use futures::TryFutureExt;
use mpt_trie::partial_trie::HashedPartialTrie;
use tokio::sync::Mutex;
use trace_decoder::trace_protocol::{
    MptBlockTraceTriePreImages, MptSeparateStorageTriesPreImage, MptSeparateTriePreImage,
    MptSeparateTriePreImages, MptTrieDirect,
};
use trace_decoder::types::HashedStorageAddr;

use super::trie::PartialTrieBuilder;

/// Processes the state witness for the given block.
pub(super) async fn process_state_witness(
    provider: Arc<Provider<Http>>,
    block: Block<H256>,
    accounts_state: Arc<Mutex<HashMap<H160, HashSet<H256>>>>,
) -> Result<MptBlockTraceTriePreImages> {
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
    )
    .await?;

    Ok(MptBlockTraceTriePreImages::Separate(
        MptSeparateTriePreImages {
            state: MptSeparateTriePreImage::Direct(MptTrieDirect(state.build())),
            storage: MptSeparateStorageTriesPreImage::MultipleTries(
                storage_proofs
                    .into_iter()
                    .map(|(a, m)| (a, MptSeparateTriePreImage::Direct(MptTrieDirect(m.build()))))
                    .collect(),
            ),
        },
    ))
}

/// Generates the state witness for the given block.
async fn generate_state_witness(
    prev_state_root: H256,
    accounts_state: HashMap<H160, HashSet<H256>>,
    provider: Arc<Provider<Http>>,
    block_number: ethereum_types::U64,
) -> Result<(
    PartialTrieBuilder<HashedPartialTrie>,
    HashMap<H256, PartialTrieBuilder<HashedPartialTrie>>,
)> {
    let mut state = PartialTrieBuilder::new(prev_state_root, Default::default());
    let mut storage_proofs =
        HashMap::<HashedStorageAddr, PartialTrieBuilder<HashedPartialTrie>>::new();

    let (account_proofs, next_account_proofs) =
        fetch_proof_data(accounts_state, provider, block_number).await?;

    // Insert account proofs
    for (address, _, proof) in account_proofs.into_iter() {
        state.insert_proof(proof.account_proof);

        let storage_mpt =
            storage_proofs
                .entry(keccak256(address).into())
                .or_insert(PartialTrieBuilder::new(
                    proof.storage_hash,
                    Default::default(),
                ));
        for proof in proof.storage_proof {
            storage_mpt.insert_proof(proof.proof);
        }
    }

    // Insert short node variants from next proofs
    for (address, _, proof) in next_account_proofs.into_iter() {
        state.insert_if_empty_short_node_variants_from_proof(proof.account_proof);

        if let Some(storage_mpt) = storage_proofs.get_mut(&keccak256(address).into()) {
            for proof in proof.storage_proof {
                storage_mpt.insert_if_empty_short_node_variants_from_proof(proof.proof);
            }
        }
    }

    Ok((state, storage_proofs))
}

async fn fetch_proof_data(
    accounts_state: HashMap<H160, HashSet<H256>>,
    provider: Arc<Provider<Http>>,
    block_number: ethereum_types::U64,
) -> Result<
    (
        Vec<(H160, HashSet<H256>, EIP1186ProofResponse)>,
        Vec<(H160, HashSet<H256>, EIP1186ProofResponse)>,
    ),
    anyhow::Error,
> {
    let account_proofs_fut = {
        let accounts_state = accounts_state.clone();
        stream::iter(accounts_state.into_iter()).then(|(address, keys)| {
            let provider = Arc::clone(&provider);
            let block_number = block_number;
            async move {
                let proof = provider
                    .get_proof(
                        address,
                        keys.iter().copied().collect(),
                        Some((block_number - 1).into()),
                    )
                    .map_err(|e| anyhow!("Failed to get proof for account: {:?}", e))
                    .await?;
                Ok::<_, anyhow::Error>((address, keys, proof))
            }
        })
    };

    let next_account_proofs_fut =
        stream::iter(accounts_state.into_iter()).then(|(address, keys)| {
            let provider = Arc::clone(&provider);
            let block_number = block_number;
            async move {
                let proof = provider
                    .get_proof(
                        address,
                        keys.iter().copied().collect(),
                        Some((block_number).into()),
                    )
                    .map_err(|e| anyhow!("Failed to get proof for account: {:?}", e))
                    .await?;
                Ok::<_, anyhow::Error>((address, keys, proof))
            }
        });

    Ok(futures::try_join!(
        account_proofs_fut.try_collect::<Vec<_>>(),
        next_account_proofs_fut.try_collect::<Vec<_>>()
    )?)
}
