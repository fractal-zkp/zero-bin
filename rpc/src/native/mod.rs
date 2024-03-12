use std::{collections::HashMap, sync::Arc};

use alloy::{providers::Provider, rpc::types::eth::BlockId, transports::Transport};
use futures::try_join;
use prover::ProverInput;

mod block;
mod state;
mod txn;

type CodeDb = HashMap<primitive_types::H256, Vec<u8>>;

/// Fetches the prover input for the given BlockId.
pub async fn prover_input<ProviderT, TransportT>(
    provider: Arc<ProviderT>,
    block_number: BlockId,
    checkpoint_block_number: BlockId,
) -> anyhow::Result<ProverInput>
where
    ProviderT: Provider<TransportT>,
    TransportT: Transport + Clone,
{
    let (block_trace, other_data) = try_join!(
        block::process_block_trace(provider.clone(), block_number),
        crate::metadata::fetch_other_block_data(provider, block_number, checkpoint_block_number,)
    )?;

    Ok(ProverInput {
        block_trace,
        other_data,
    })
}
