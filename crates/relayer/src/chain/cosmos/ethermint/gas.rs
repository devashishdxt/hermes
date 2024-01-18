use ibc_proto::ethermint::evm::v1::DynamicFeeTx;
use serde_json::json;
use tracing::trace;

use crate::{chain::cosmos::types::config::TxConfig, error::Error, keyring::Secp256k1KeyPair};

/// Estimates the gas for the given `DynamicFeeTx`.
pub async fn estimate_gas(
    dynamic_fee_tx: &DynamicFeeTx,
    key_pair: &Secp256k1KeyPair,
    config: &TxConfig,
) -> Result<u64, Error> {
    let from = format!("0x{}", hex::encode(key_pair.address()));
    let gas = format!("0x{:x}", dynamic_fee_tx.gas);
    let data = format!("0x{}", hex::encode(&dynamic_fee_tx.data));

    let params = json!([{
        "from": from,
        "to": dynamic_fee_tx.to,
        "gas": gas,
        "input": data,
    }]);

    trace!("estimate gas params: {}", params);

    let request = json!({
        "jsonrpc": "2.0",
        "method": "eth_estimateGas",
        "params": params,
        "id": 1,
    });

    let client = reqwest::Client::new();

    let response: serde_json::Value = client
        .post(config.json_rpc_address.as_ref().unwrap().to_string())
        .json(&request)
        .send()
        .await
        .map_err(|e| Error::reqwest_error(format!("{:?}", e)))?
        .json()
        .await
        .map_err(|e| Error::reqwest_error(format!("{:?}", e)))?;

    let result = response
        .get("result")
        .ok_or_else(|| Error::ethermint_error(format!("failed to estimate gas: {}", response)))?
        .as_str()
        .ok_or_else(|| Error::ethermint_error(format!("invalid estimated gas: {}", response)))?;

    let estimated_gas = estimated_gas_hex_to_u64(result)?;

    trace!("estimated gas: {}", estimated_gas);

    Ok(estimated_gas)
}

/// Converts hex encoded estimated gas to u64
fn estimated_gas_hex_to_u64(estimated_gas_hex: &str) -> Result<u64, Error> {
    if estimated_gas_hex.starts_with("0x") {
        u64::from_str_radix(&estimated_gas_hex[2..], 16)
            .map_err(|e| Error::from_hex_error(format!("failed to parse estimated gas: {:?}", e)))
    } else {
        u64::from_str_radix(estimated_gas_hex, 16)
            .map_err(|e| Error::from_hex_error(format!("failed to parse estimated gas: {:?}", e)))
    }
}
