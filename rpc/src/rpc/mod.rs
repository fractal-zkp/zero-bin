use anyhow::Result;
use async_trait::async_trait;
use prover::ProverInput;

mod jerigon;
pub use jerigon::JerigonRpcClient;

mod native;
pub use native::NativeRpcClient;

/// RPC CLIENT TRAIT
/// ===============================================================================================

/// The RPC client trait.
///
/// This trait defines the interface for fetching prover input from the RPC.
#[async_trait]
pub trait RpcClient {
    /// Fetches the prover input from the RPC.
    async fn fetch_prover_input(
        &self,
        block_number: u64,
        checkpoint_block_number: u64,
    ) -> Result<ProverInput>;
}
