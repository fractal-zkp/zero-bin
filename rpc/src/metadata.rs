use alloy::{
    providers::Provider,
    rpc::types::eth::{BlockId, BlockTransactionsKind, Withdrawal},
    transports::Transport,
};
use anyhow::Context as _;
use evm_arithmetization::proof::{BlockHashes, BlockMetadata};
use futures::{StreamExt as _, TryStreamExt as _};
use trace_decoder::types::{BlockLevelData, OtherBlockData};

use super::compat::ToPrimitive;

pub async fn fetch_other_block_data<ProviderT, TransportT>(
    provider: ProviderT,
    target_block_id: BlockId,
    checkpoint_block_id: BlockId,
) -> anyhow::Result<OtherBlockData>
where
    ProviderT: Provider<TransportT>,
    TransportT: Transport + Clone,
{
    let target_block = provider
        .get_block(target_block_id, BlockTransactionsKind::Hashes)
        .await?
        .context("target block does not exist")?;
    let target_block_number = target_block
        .header
        .number
        .context("target block is missing field `number`")?;
    let chain_id = provider.get_chain_id().await?;
    let checkpoint_state_trie_root = provider
        .get_block(checkpoint_block_id, BlockTransactionsKind::Hashes)
        .await?
        .context("checkpoint block does not exist")?
        .header
        .state_root;

    let mut prev_hashes = [alloy::primitives::B256::ZERO; 256];
    let concurrency = prev_hashes.len();
    futures::stream::iter(
        prev_hashes
            .iter_mut()
            .rev() // fill RTL
            .zip(std::iter::successors(Some(target_block_number), |it| {
                it.checked_sub(1)
            }))
            .map(|(dst, n)| {
                let provider = &provider;
                async move {
                    let block = provider
                        .get_block(n.into(), BlockTransactionsKind::Hashes)
                        .await
                        .context("couldn't get block")?
                        .context("no such block")?;
                    *dst = block.header.parent_hash;
                    anyhow::Ok(())
                }
            }),
    )
    .buffered(concurrency)
    .try_collect::<()>()
    .await
    .context("couldn't fill previous hashes")?;

    let other_data = OtherBlockData {
        b_data: BlockLevelData {
            b_meta: BlockMetadata {
                block_beneficiary: target_block.header.miner.to_primitive(),
                block_timestamp: target_block.header.timestamp.into(),
                block_number: target_block_number.into(),
                block_difficulty: target_block.header.difficulty.into(),
                block_random: target_block
                    .header
                    .mix_hash
                    .context("target block is missing field `mix_hash`")?
                    .to_primitive(),
                block_gaslimit: target_block.header.gas_limit.into(),
                block_chain_id: chain_id.into(),
                block_base_fee: target_block
                    .header
                    .base_fee_per_gas
                    .context("target block is missing field `base_fee_per_gas`")?
                    .into(),
                block_gas_used: target_block.header.gas_used.into(),
                block_bloom: target_block.header.logs_bloom.to_primitive(),
            },
            b_hashes: BlockHashes {
                prev_hashes: prev_hashes.map(|it| it.to_primitive()).into(),
                cur_hash: target_block
                    .header
                    .hash
                    .context("target block is missing field `hash`")?
                    .to_primitive(),
            },
            withdrawals: target_block
                .withdrawals
                .into_iter()
                .flatten()
                .map(
                    |Withdrawal {
                         address, amount, ..
                     }| { (address.to_primitive(), amount.into()) },
                )
                .collect(),
        },
        checkpoint_state_trie_root: checkpoint_state_trie_root.to_primitive(),
    };
    Ok(other_data)
}
