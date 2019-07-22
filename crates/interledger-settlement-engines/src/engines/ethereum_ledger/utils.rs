use ethabi::Token;
use ethereum_tx_sign::{
    web3::types::{Address, Transaction, U256},
    RawTransaction,
};

// Helper function which is used to construct an Ethereum transaction sending
// `value` tokens to `to`. The account's nonce is required since Ethereum uses
// an account based model with nonces for replay protection. If a
// `token_address` is provided, then an ERC20 transaction is created instead for
// that token. Ethereum transactions cost 21000 Gas, while ERC20 transactions
// cost at most 70k (can tighten the gas limit, but 70k is safe if the address
// is indeed an ERC20 token.
// TODO: pass it gas_price as a parameter which is calculated from `web3.eth().gas_price()`
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
