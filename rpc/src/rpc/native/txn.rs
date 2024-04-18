use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use ethers::providers::Middleware;
use ethers::providers::{Http, Provider};
use ethers::types::{
    Address, DiffMode, GethTrace, GethTraceFrame, PreStateFrame, PreStateMode, TransactionReceipt,
    H160, H256, U256,
};
use ethers::utils::keccak256;
use ethers::utils::rlp;
use tokio::sync::Mutex;
use trace_decoder::trace_protocol::ContractCodeUsage;
use trace_decoder::trace_protocol::TxnTrace;
use trace_decoder::trace_protocol::{TxnInfo, TxnMeta};

use super::tracing_options;
use super::tracing_options_diff;

/// Processes the transaction with the given transaction hash and updates the
/// accounts state.
pub(super) async fn process_transaction(
    provider: Arc<Provider<Http>>,
    tx_hash: &H256,
    accounts_state: Arc<Mutex<HashMap<H160, HashSet<H256>>>>,
    code_db: Arc<Mutex<HashMap<H256, Vec<u8>>>>,
) -> Result<TxnInfo> {
    let (tx, tx_receipt, pre_trace, diff_trace) = fetch_tx_data(provider, tx_hash).await?;
    let tx_meta = TxnMeta {
        byte_code: tx.rlp().to_vec(),
        new_txn_trie_node_byte: tx.rlp().to_vec(),
        new_receipt_trie_node_byte: compute_receipt_bytes(&tx_receipt),
        gas_used: tx_receipt.gas_used.unwrap().as_u64(),
    };

    let mut accounts_state = accounts_state.lock().await;
    let mut code_db = code_db.lock().await;
    let tx_traces = match (pre_trace, diff_trace) {
        (
            GethTrace::Known(GethTraceFrame::PreStateTracer(PreStateFrame::Default(read))),
            GethTrace::Known(GethTraceFrame::PreStateTracer(PreStateFrame::Diff(diff))),
        ) => process_tx_traces(&mut accounts_state, &mut code_db, read, diff)?,
        _ => unreachable!(),
    };

    Ok(TxnInfo {
        meta: tx_meta,
        traces: tx_traces,
    })
}

/// Fetches the transaction data for the given transaction hash.
async fn fetch_tx_data(
    provider: Arc<Provider<Http>>,
    tx_hash: &H256,
) -> Result<
    (
        ethers::types::Transaction,
        TransactionReceipt,
        GethTrace,
        GethTrace,
    ),
    anyhow::Error,
> {
    let tx_fut = provider.get_transaction(*tx_hash);
    let tx_receipt_fut = provider.get_transaction_receipt(*tx_hash);
    let pre_trace_fut = provider.debug_trace_transaction(*tx_hash, tracing_options());
    let diff_trace_fut = provider.debug_trace_transaction(*tx_hash, tracing_options_diff());

    let (tx, tx_receipt, pre_trace, diff_trace) =
        futures::try_join!(tx_fut, tx_receipt_fut, pre_trace_fut, diff_trace_fut,)?;

    Ok((
        tx.ok_or_else(|| anyhow!("Transaction not found."))?,
        tx_receipt.ok_or_else(|| anyhow!("Transaction receipt not found."))?,
        pre_trace,
        diff_trace,
    ))
}

// TODO: upstream this to ethers
/// Computes the receipt bytes for the given transaction receipt.
///
/// NOTE: The ethers-rs library does not encode the transaction type in the
/// transaction receipt hence we do it manually here.
fn compute_receipt_bytes(tx_receipt: &TransactionReceipt) -> Vec<u8> {
    let mut bytes = rlp::encode(tx_receipt).to_vec();

    match tx_receipt.transaction_type {
        Some(tx_type) if !tx_type.is_zero() => {
            bytes.insert(0, tx_type.0[0] as u8);
        }
        _ => return bytes,
    }

    rlp::encode(&bytes).to_vec()
}

/// Processes the transaction traces and updates the accounts state.
fn process_tx_traces(
    accounts_state: &mut HashMap<H160, HashSet<H256>>,
    code_db: &mut HashMap<H256, Vec<u8>>,
    read_trace: PreStateMode,
    diff_trace: DiffMode,
) -> Result<HashMap<Address, TxnTrace>> {
    let DiffMode {
        pre: pre_trace,
        post: post_trace,
    } = diff_trace;

    let mut addresses = HashSet::<H160>::new();
    addresses.extend(read_trace.0.keys());
    addresses.extend(post_trace.keys());
    addresses.extend(pre_trace.keys());

    Ok(addresses
        .into_iter()
        .map(|address| {
            let storage_keys = accounts_state.entry(address).or_insert(Default::default());

            let acct_state = read_trace.0.get(&address);
            let balance = post_trace.get(&address).and_then(|x| x.balance);
            let nonce = post_trace.get(&address).and_then(|x| x.nonce);

            let storage_read = acct_state.and_then(|acct| {
                acct.storage.as_ref().map(|x| {
                    let read_keys: Vec<H256> = x.keys().copied().collect();
                    storage_keys.extend(read_keys.iter().copied());
                    read_keys
                })
            });

            let storage_written = post_trace.get(&address).and_then(|x| {
                x.storage.as_ref().map(|s| {
                    let write_keys: HashMap<H256, U256> = s
                        .iter()
                        //TODO: check if endianess is correct
                        .map(|(k, v)| (*k, U256::from_big_endian(&v.0)))
                        .collect();
                    storage_keys.extend(write_keys.keys().copied());
                    write_keys
                })
            });

            let code_usage = post_trace
                .get(&address)
                .and_then(|x| x.code.as_ref())
                .map_or(
                    acct_state.and_then(|acct| {
                        acct.code.as_ref().map(|x| {
                            let code = hex::decode(&x[2..]).expect("must be valid");
                            let code_hash = keccak256(&code).into();
                            code_db.insert(code_hash, code);
                            ContractCodeUsage::Read(code_hash)
                        })
                    }),
                    |x| {
                        let code = hex::decode(&x[2..]).expect("must be valid");
                        let code_hash = keccak256(&code).into();
                        code_db.insert(code_hash, code.clone());
                        Some(ContractCodeUsage::Write(code.into()))
                    },
                );

            let self_destructed =
                if post_trace.get(&address).is_none() && pre_trace.contains_key(&address) {
                    Some(true)
                } else {
                    None
                };
            (
                address,
                TxnTrace {
                    balance,
                    nonce,
                    storage_read,
                    storage_written,
                    code_usage,
                    self_destructed,
                },
            )
        })
        .collect::<HashMap<H160, TxnTrace>>())
}
