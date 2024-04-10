use std::collections::BTreeMap;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use ethers::providers::Middleware;
use ethers::providers::{Http, Provider};
use ethers::types::U256;
use ethers::types::{
    AccountState, Address, DiffMode, GethTrace, GethTraceFrame, PreStateFrame, PreStateMode,
    TransactionReceipt, H160, H256,
};
use ethers::utils::rlp;
use trace_decoder::trace_protocol::ContractCodeUsage;
use trace_decoder::trace_protocol::TxnTrace;
use trace_decoder::trace_protocol::{TxnInfo, TxnMeta};

use super::tracing_options;
use super::tracing_options_diff;

/// Processes the transaction with the given transaction hash and updates the
/// accounts state.
pub(super) async fn process_transaction(
    provider: &Provider<Http>,
    tx_hash: &H256,
    accounts_state: Arc<Mutex<BTreeMap<H160, AccountState>>>,
) -> Result<TxnInfo> {
    let (tx, tx_receipt, pre_trace, diff_trace) = fetch_tx_data(provider, tx_hash).await?;
    let tx_meta = TxnMeta {
        byte_code: tx.rlp().to_vec(),
        new_txn_trie_node_byte: tx.rlp().to_vec(),
        new_receipt_trie_node_byte: compute_receipt_bytes(&tx_receipt),
        gas_used: tx
            .gas
            .try_into()
            .map_err(|_| anyhow!("gas used must be valid u64"))?,
    };

    let tx_traces = match (pre_trace, diff_trace) {
        (
            GethTrace::Known(GethTraceFrame::PreStateTracer(PreStateFrame::Default(read))),
            GethTrace::Known(GethTraceFrame::PreStateTracer(PreStateFrame::Diff(diff))),
        ) => process_tx_traces(accounts_state, read, diff).await?,
        _ => unreachable!(),
    };

    Ok(TxnInfo {
        meta: tx_meta,
        traces: tx_traces,
    })
}

/// Fetches the transaction data for the given transaction hash.
async fn fetch_tx_data(
    provider: &Provider<Http>,
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
async fn process_tx_traces(
    accounts_state: Arc<Mutex<BTreeMap<H160, AccountState>>>,
    read_trace: PreStateMode,
    diff_trace: DiffMode,
) -> Result<HashMap<Address, TxnTrace>> {
    let mut accounts_state = accounts_state.lock().unwrap();
    for (address, acct_state) in read_trace.0.iter() {
        accounts_state
            .entry(*address)
            .and_modify(|acct| {
                merge_account_storage(&mut acct.storage, acct_state.storage.as_ref())
            })
            .or_insert(acct_state.clone());
    }

    let DiffMode {
        pre: pre_trace,
        post: post_trace,
    } = diff_trace;

    Ok(read_trace
        .0
        .into_iter()
        .map(|(address, acct_state)| {
            let balance = post_trace.get(&address).and_then(|x| x.balance);
            let nonce = post_trace.get(&address).and_then(|x| x.nonce);
            let storage_read = acct_state.storage.map(|x| x.keys().copied().collect());

            let storage_written = post_trace.get(&address).and_then(|x| {
                x.storage.as_ref().map(|s| {
                    s.iter()
                        //TODO: check if endianess is correct
                        .map(|(k, v)| (*k, U256::from_big_endian(v.as_bytes())))
                        .collect()
                })
            });

            let code_usage = post_trace.get(&address).map_or(
                acct_state
                    .code
                    .map(|x| ContractCodeUsage::Read(H256::from_str(&x).expect("must be valid"))),
                |x| {
                    x.code
                        .as_ref()
                        .map(|x| ContractCodeUsage::Write(x.as_bytes().to_vec().into()))
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

/// Merges the account storage.
fn merge_account_storage(
    storage: &mut Option<BTreeMap<H256, H256>>,
    new_storage: Option<&BTreeMap<H256, H256>>,
) {
    match (storage, new_storage) {
        (Some(storage), Some(new_storage)) => storage.extend(new_storage),
        (storage, Some(new_storage)) => {
            *storage = Some(new_storage.clone());
        }
        _ => (),
    }
}
