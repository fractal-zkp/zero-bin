use std::{
    fs::{create_dir_all, File},
    io::Write,
    path::PathBuf,
    sync::Arc,
};

use alloy::transports::http::reqwest::Url;
use paladin::runtime::Runtime;
use proof_gen::types::PlonkyProofIntern;
use rpc::retry::build_http_retry_provider;

/// The main function for the jerigon mode.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn rpc_main(
    rpc_type: &str,
    rpc_url: Url,
    runtime: Runtime,
    block_number: u64,
    checkpoint_block_number: u64,
    previous: Option<PlonkyProofIntern>,
    proof_output_path_opt: Option<PathBuf>,
    save_inputs_on_error: bool,
    backoff: u64,
    max_retries: u32,
) -> anyhow::Result<()> {
    let prover_input = match rpc_type {
        "jerigon" => {
            rpc::jerigon::prover_input(
                build_http_retry_provider(rpc_url, backoff, max_retries),
                block_number.into(),
                checkpoint_block_number.into(),
            )
            .await?
        }
        "native" => {
            rpc::native::prover_input(
                Arc::new(build_http_retry_provider(rpc_url, backoff, max_retries)),
                block_number.into(),
                checkpoint_block_number.into(),
            )
            .await?
        }
        _ => unreachable!(),
    };

    let proof = prover_input
        .prove(&runtime, previous, save_inputs_on_error)
        .await;
    runtime.close().await?;

    let proof = serde_json::to_vec(&proof?.intern)?;
    write_proof(proof, proof_output_path_opt)
}

fn write_proof(proof: Vec<u8>, proof_output_path_opt: Option<PathBuf>) -> anyhow::Result<()> {
    match proof_output_path_opt {
        Some(p) => {
            if let Some(parent) = p.parent() {
                create_dir_all(parent)?;
            }

            let mut f = File::create(p)?;
            f.write_all(&proof)?;
        }
        None => std::io::stdout().write_all(&proof)?,
    }

    Ok(())
}
