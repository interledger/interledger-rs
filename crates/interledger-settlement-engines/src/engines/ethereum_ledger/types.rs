use ethereum_tx_sign::{
    web3::types::{Address, H256, U256},
    RawTransaction,
};
use ethkey::KeyPair;
use futures::Future;
use interledger_service::Account;
use parity_crypto::Keccak256;
use std::str::FromStr;

/// An Ethereum account is associated with an address. We additionally require
/// that an optional `token_address` is implemented. If the `token_address` of an
/// Ethereum Account is not `None`, than that account is used with the ERC20 token
/// associated with that `token_address`.
pub trait EthereumAccount: Account {
    fn own_address(&self) -> Address;

    fn token_address(&self) -> Option<Address> {
        None
    }
}

#[derive(Debug, Extract, Serialize, Deserialize, Clone, PartialEq, Eq, Hash, Copy)]
pub struct Addresses {
    pub own_address: Address,
    pub token_address: Option<Address>,
}

/// Trait used to store Ethereum account addresses, as well as any data related
/// to the connector notifier service such as the most recently observed block
/// and account balance
pub trait EthereumStore {
    type Account: EthereumAccount;

    /// Saves the Ethereum address associated with this account
    /// called when creating an account on the API.
    fn save_account_addresses(
        &self,
        account_ids: Vec<<Self::Account as Account>::AccountId>,
        data: Vec<Addresses>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    /// Loads the Ethereum address associated with this account
    fn load_account_addresses(
        &self,
        account_ids: Vec<<Self::Account as Account>::AccountId>,
    ) -> Box<dyn Future<Item = Vec<Addresses>, Error = ()> + Send>;

    /// Saves the latest block number and account balance, up to which all
    /// transactions have been communicated to the connector
    fn save_recently_observed_data(
        &self,
        block: U256,
        balance: U256,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    /// Loads the latest saved block number and account balance
    fn load_recently_observed_data(
        &self,
    ) -> Box<dyn Future<Item = (U256, U256), Error = ()> + Send>;

    /// Retrieves the account id associated with the provided addresses pair.
    /// Note that an account with Addresses(0x1, None) must have different id
    /// from an account with Addresses(0x1, 0x2), since the first is 0x1 on
    /// Ether, while the second one is 0x1 on some ERC20 token.
    fn load_account_id_from_address(
        &self,
        eth_address: Addresses,
    ) -> Box<dyn Future<Item = <Self::Account as Account>::AccountId, Error = ()> + Send>;
}

/// Implement this trait for datatypes which can be used to sign an Ethereum
/// Transaction, e.g. an HSM, Ledger, Trezor connection, or a private key
/// string.
/// TODO: All methods should be converted to return a Future, since an HSM
/// connection is asynchronous
pub trait EthereumLedgerTxSigner {
    /// Takes a transaction and returns an RLP encoded signed version of it
    fn sign(&self, tx: RawTransaction, chain_id: u8) -> Vec<u8>;

    /// Returns the Ethereum address associated with the signer
    fn address(&self) -> Address;
}

impl EthereumLedgerTxSigner for String {
    fn sign(&self, tx: RawTransaction, chain_id: u8) -> Vec<u8> {
        tx.sign(&H256::from_str(self).unwrap(), &chain_id)
    }

    fn address(&self) -> Address {
        let keypair = KeyPair::from_secret(self.parse().unwrap()).unwrap();
        let public = keypair.public();
        let hash = public.keccak256();
        Address::from(&hash[12..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_address() {
        let privkey =
            String::from("acb8f4184aaf6490b6e6aea7b474225be0d965eed75f4b91183eff6032c299f8");
        let addr = privkey.address();
        assert_eq!(
            addr,
            Address::from("4070abbd2e38a8d27cd5a495f482c13f049f8310")
        );
    }
}
