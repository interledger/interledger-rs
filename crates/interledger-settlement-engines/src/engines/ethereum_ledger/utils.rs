use ethabi::Token;
use ethereum_tx_sign::{
    web3::{
        api::Web3,
        futures::future::Future,
        transports::Http,
        types::{Address, BlockNumber, FilterBuilder, Transaction, H160, H256, U256},
    },
    RawTransaction,
};
use log::error;
use std::str::FromStr;

/// This is the result of keccak256("Transfer(address,address,to)"), which is
/// used to filter through Ethereum ERC20 Transfer events in transaction receipts.
const TRANSFER_EVENT_FILTER: &str =
    "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

// Helper function which is used to construct an Ethereum transaction sending
// `value` tokens to `to`. If a `token_address` is provided, then an ERC20
// transaction  is created instead for that token. The `nonce`, `gas` and
// `gas_price` fields are set to 0 and are expected to be set with the values
// returned by the corresponding `eth_getTransactionCount`, `eth_estimateGas`,
// `eth_gasPrice` calls to an Ethereum node.
pub fn make_tx(to: Address, value: U256, token_address: Option<Address>) -> RawTransaction {
    if let Some(token_address) = token_address {
        // Ethereum contract transactions format:
        // [transfer function selector][`to` padded ][`value` padded]
        // transfer function selector: sha3("transfer(to,address)")[0:8] =
        // "a9059cbb"
        // The actual receiver of the transaction is the ERC20 `token_address`
        // The value of the transaction is 0 wei since we are transferring an ERC20
        let mut data = hex::decode("a9059cbb").unwrap();
        data.extend(ethabi::encode(&[Token::Address(to), Token::Uint(value)]));
        RawTransaction {
            to: Some(token_address),
            nonce: U256::from(0),
            data,
            gas: U256::from(0),
            gas_price: U256::from(0),
            value: U256::zero(),
        }
    } else {
        // Ethereum account transaction:
        // The receiver is `to`, and the data field is left empty.
        RawTransaction {
            to: Some(to),
            nonce: U256::from(0),
            data: vec![],
            gas: U256::from(0),
            gas_price: U256::from(0),
            value,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ERC20Transfer {
    pub tx_hash: H256,
    pub from: Address,
    pub to: Address,
    pub amount: U256,
}

/// Filters out transactions where the `from` and `to` fields match the provides
/// addreses.
pub fn transfer_logs(
    web3: Web3<Http>,
    contract_address: Address,
    from: Option<Address>,
    to: Option<Address>,
    from_block: BlockNumber,
    to_block: BlockNumber,
) -> impl Future<Item = Vec<ERC20Transfer>, Error = ()> {
    let from = if let Some(from) = from {
        Some(vec![H256::from(from)])
    } else {
        None
    };
    let to = if let Some(to) = to {
        Some(vec![H256::from(to)])
    } else {
        None
    };

    // create a filter for Transfer events from `from_block` until `to_block
    // that filters all events indexed by `from` and `to`.
    let filter = FilterBuilder::default()
        .from_block(from_block)
        .to_block(to_block)
        .address(vec![contract_address])
        .topics(
            Some(vec![H256::from(
                // keccak256("transfer(address,address,to)")
                TRANSFER_EVENT_FILTER,
            )]),
            from,
            to,
            None,
        )
        .build();

    // Make an eth_getLogs call to the Ethereum node
    web3.eth()
        .logs(filter)
        .map_err(move |err| error!("Got error when fetching transfer logs{:?}", err))
        .and_then(move |logs| {
            let mut ret = Vec::new();
            for log in logs {
                // NOTE: From/to are indexed events.
                // Amount is parsed directly from the data field.
                let indexed = log.topics;
                let from = H160::from(indexed[1]);
                let to = H160::from(indexed[2]);
                let data = log.data;
                let amount = U256::from_str(&hex::encode(data.0)).unwrap();
                ret.push(ERC20Transfer {
                    tx_hash: log.transaction_hash.unwrap(),
                    from,
                    to,
                    amount,
                });
            }
            Ok(ret)
        })
}

// TODO: Extend this function to inspect the data field of a
// transaction, so that it supports contract wallets such as the Gnosis Multisig
// etc. There is no need to implement any ERC20 functionality here since these
// transfers can be quickly found by filtering for the `Transfer` ERC20 event.
pub fn sent_to_us(tx: Transaction, our_address: Address) -> (Address, U256, Option<Address>) {
    if let Some(to) = tx.to {
        if tx.value > U256::from(0) && to == our_address {
            return (tx.from, tx.value, None);
        }
    }
    (tx.from, U256::from(0), None) // if it's not for us the amount is 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_erc20_make_tx() {
        // https://etherscan.io/tx/0x6fd1b68f02f4201a38662647b7f09170b159faec6af4825ae509beefeb8e8130
        let to = "c92be489639a9c61f517bd3b955840fa19bc9b7c".parse().unwrap();
        let value = "16345785d8a0000".into();
        let token_address = Some("B8c77482e45F1F44dE1745F52C74426C631bDD52".into());
        let tx = make_tx(to, value, token_address);
        assert_eq!(tx.to, token_address);
        assert_eq!(tx.value, U256::from(0));
        assert_eq!(hex::encode(tx.data), "a9059cbb000000000000000000000000c92be489639a9c61f517bd3b955840fa19bc9b7c000000000000000000000000000000000000000000000000016345785d8a0000")
    }

    #[test]
    fn test_eth_make_tx() {
        let to = "c92be489639a9c61f517bd3b955840fa19bc9b7c".parse().unwrap();
        let value = "16345785d8a0000".into();
        let token_address = None;
        let tx = make_tx(to, value, token_address);
        assert_eq!(tx.to, Some(to));
        assert_eq!(tx.value, value);
        let empty: Vec<u8> = Vec::new();
        assert_eq!(tx.data, empty);
    }

}
