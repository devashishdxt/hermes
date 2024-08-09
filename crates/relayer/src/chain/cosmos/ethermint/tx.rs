use std::{thread, time::Duration};

use ibc_proto::{
    cosmos::{
        base::v1beta1::Coin,
        tx::v1beta1::{AuthInfo, Fee, TxBody, TxRaw},
    },
    ethermint::evm::v1::{DynamicFeeTx, MsgEthereumTx},
    google::protobuf::Any,
};
use primitive_types::U256;
use prost::Message;

use crate::{
    chain::cosmos::{
        estimate::EstimatedGas,
        gas::{adjust_estimated_gas, AdjustGas},
        types::{account::Account, config::TxConfig},
    },
    error::Error,
    keyring::Secp256k1KeyPair,
};

use super::{
    batch_call::pack_batch_call_data,
    gas::estimate_gas,
    hash::get_transaction_raw,
    sign::sign_dynamic_fee_tx,
    util::{get_evm_extension_options, get_gas_tip_cap, parse_chain_id},
};

/// Builds the `TxRaw` for the given messages.
pub async fn build_tx_raw(
    config: &TxConfig,
    key_pair: &Secp256k1KeyPair,
    account: &Account,
    messages: &[Any],
) -> Result<(TxRaw, EstimatedGas), Error> {
    let (mut dynamic_fee_tx, estimated_gas) =
        build_dynamic_fee_tx(messages, account, key_pair, config).await?;
    sign_dynamic_fee_tx(&mut dynamic_fee_tx, key_pair)?;

    let eth_tx = build_ethereum_tx(&dynamic_fee_tx, key_pair)?;
    let eth_tx_any = get_ethereum_tx_any(&eth_tx);

    let tx_body = TxBody {
        messages: vec![eth_tx_any],
        memo: "".to_string(),
        timeout_height: 0,
        extension_options: get_evm_extension_options(),
        non_critical_extension_options: Vec::new(),
    };

    let auth_info = AuthInfo {
        signer_infos: Vec::new(),
        fee: Some(Fee {
            amount: vec![Coin {
                denom: config.gas_config.gas_price.denom.clone(),
                amount: (U256::from_dec_str(&dynamic_fee_tx.gas_fee_cap).unwrap()
                    * dynamic_fee_tx.gas)
                    .to_string(),
            }],
            gas_limit: dynamic_fee_tx.gas,
            payer: "".to_string(),
            granter: "".to_string(),
        }),
        tip: None,
    };

    Ok((
        TxRaw {
            body_bytes: tx_body.encode_to_vec(),
            auth_info_bytes: auth_info.encode_to_vec(),
            signatures: Vec::new(),
        },
        estimated_gas,
    ))
}

/// Builds the `DynamicFeeTx` for the given message with retry logic.
async fn build_dynamic_fee_tx(
    messages: &[Any],
    account: &Account,
    key_pair: &Secp256k1KeyPair,
    config: &TxConfig,
) -> Result<(DynamicFeeTx, EstimatedGas), Error> {
    let nonce = account.sequence.to_u64();

    let mut tx = DynamicFeeTx::default();

    tx.chain_id = parse_chain_id(config.chain_id.as_str())?;
    tx.nonce = nonce;
    tx.to = config
        .precompiled_contract_address
        .as_ref()
        .unwrap()
        .clone();
    tx.value = "0".to_string();
    tx.data = pack_batch_call_data(
        messages,
        config.precompiled_contract_address.as_ref().unwrap(),
        &config.account_prefix,
    )?;
    tx.gas_fee_cap = config.gas_config.gas_price.price.to_string();
    tx.gas = config.gas_config.max_gas;
    tx.accesses = Vec::new();

    if let Some(gas_tip_cap) = get_gas_tip_cap(config)? {
        tx.gas_tip_cap = gas_tip_cap;
    }

    if !config.json_rpc_address.is_some() {
        return Ok((tx, EstimatedGas::Default(config.gas_config.max_gas)))
    }

    let max_retries = 5;
    let mut retries = 0;
    loop {
        match estimate_gas(&tx, key_pair, config).await {
            Ok(estimated_gas) => {
                let adjusted_gas = adjust_estimated_gas(AdjustGas {
                    gas_multiplier: config.gas_config.gas_multiplier,
                    max_gas: config.gas_config.max_gas,
                    gas_amount: estimated_gas,
                });

                if adjusted_gas > config.gas_config.max_gas {
                    return Err(Error::tx_simulate_gas_estimate_exceeded(
                        config.chain_id.clone(),
                        adjusted_gas,
                        config.gas_config.max_gas,
                    ));
                }

                tx.gas = adjusted_gas;

                return Ok((tx, EstimatedGas::Simulated(adjusted_gas)))
            }
            Err(e) => {
                retries += 1;
                tracing::error!("failed to estimate ethermint gas: {:?} after {:?} retries", e, retries);

                if retries >= max_retries {
                    tracing::error!("failed to estimate ethermint gas after max retries");
                    return Ok((tx, EstimatedGas::Default(config.gas_config.max_gas)));
                }

                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

/// Builds the `MsgEthereumTx` for the given `DynamicFeeTx`.
fn build_ethereum_tx(
    dynamic_fee_tx: &DynamicFeeTx,
    key_pair: &Secp256k1KeyPair,
) -> Result<MsgEthereumTx, Error> {
    // let dynamic_fee_tx_any = Any {
    //     type_url: "/ethermint.evm.v1.DynamicFeeTx".to_string(),
    //     value: dynamic_fee_tx.encode_to_vec(),
    // };

    let mut eth_tx = MsgEthereumTx::default();
    // eth_tx.data = Some(dynamic_fee_tx_any);
    eth_tx.from = key_pair.address().to_vec();
    // eth_tx.deprecated_hash = get_transaction_hash(&dynamic_fee_tx)?;
    eth_tx.raw = get_transaction_raw(&dynamic_fee_tx)?;

    Ok(eth_tx)
}

/// Converts the given `MsgEthereumTx` to `Any`.
fn get_ethereum_tx_any(eth_tx: &MsgEthereumTx) -> Any {
    Any {
        type_url: "/ethermint.evm.v1.MsgEthereumTx".to_string(),
        value: eth_tx.encode_to_vec(),
    }
}
