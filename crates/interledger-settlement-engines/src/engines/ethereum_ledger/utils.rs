use ethabi::Token;
use ethereum_tx_sign::{
    web3::types::{Address, U256},
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
pub fn make_tx(
    to: Address,
    value: U256,
    nonce: U256,
    token_address: Option<Address>,
) -> RawTransaction {
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
            nonce,
            data,
            gas: 70000.into(), // ERC20 transactions cost approximately 40k gas.
            gas_price: 20000.into(),
            value: U256::zero(),
        }
    } else {
        // Ethereum account transaction:
        // The receiver is `to`, and the data field is left empty.
        RawTransaction {
            to: Some(to),
            nonce,
            data: vec![],
            gas: 21000.into(),
            gas_price: 20000.into(),
            value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_erc20_make_tx() {
        // https://etherscan.io/tx/0x6fd1b68f02f4201a38662647b7f09170b159faec6af4825ae509beefeb8e8130
        let to = "c92be489639a9c61f517bd3b955840fa19bc9b7c".parse().unwrap();
        let value = "16345785d8a0000".into();
        let nonce = 1.into();
        let token_address = Some("B8c77482e45F1F44dE1745F52C74426C631bDD52".into());
        let tx = make_tx(to, value, nonce, token_address);
        assert_eq!(tx.to, token_address);
        assert_eq!(tx.value, U256::from(0));
        assert_eq!(hex::encode(tx.data), "a9059cbb000000000000000000000000c92be489639a9c61f517bd3b955840fa19bc9b7c000000000000000000000000000000000000000000000000016345785d8a0000")
    }

    #[test]
    fn test_eth_make_tx() {
        let to = "c92be489639a9c61f517bd3b955840fa19bc9b7c".parse().unwrap();
        let value = "16345785d8a0000".into();
        let nonce = 1.into();
        let token_address = None;
        let tx = make_tx(to, value, nonce, token_address);
        assert_eq!(tx.to, Some(to));
        assert_eq!(tx.value, value);
        let empty: Vec<u8> = Vec::new();
        assert_eq!(tx.data, empty);
    }

}
