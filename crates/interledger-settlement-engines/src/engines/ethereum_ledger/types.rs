use clarity::{PrivateKey, Signature};
use ethereum_tx_sign::RawTransaction;
use futures::Future;
use sha3::{Digest, Keccak256 as Sha3};
use std::collections::HashMap;
use std::str::FromStr;
use web3::types::{Address, H256, U256};

use serde::Serialize;
use std::fmt::{Debug, Display};
/// An Ethereum account is associated with an address. We additionally require
/// that an optional `token_address` is implemented. If the `token_address` of an
/// Ethereum Account is not `None`, than that account is used with the ERC20 token
/// associated with that `token_address`.
///
use std::hash::Hash;
pub trait EthereumAccount {
    type AccountId: Eq
        + Hash
        + Debug
        + Display
        + Default
        + FromStr
        + Send
        + Sync
        + Clone
        + Serialize;

    fn id(&self) -> Self::AccountId;

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
        data: HashMap<<Self::Account as EthereumAccount>::AccountId, Addresses>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    /// Loads the Ethereum address associated with this account
    fn load_account_addresses(
        &self,
        account_ids: Vec<<Self::Account as EthereumAccount>::AccountId>,
    ) -> Box<dyn Future<Item = Vec<Addresses>, Error = ()> + Send>;

    /// Saves the latest block number, up to which all
    /// transactions have been communicated to the connector
    fn save_recently_observed_block(
        &self,
        block: U256,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    /// Loads the latest saved block number
    fn load_recently_observed_block(
        &self,
    ) -> Box<dyn Future<Item = Option<U256>, Error = ()> + Send>;

    /// Retrieves the account id associated with the provided addresses pair.
    /// Note that an account with the same `own_address` but different ERC20
    /// `token_address` can exist multiple times since each occurence represents
    /// a different token.
    fn load_account_id_from_address(
        &self,
        eth_address: Addresses,
    ) -> Box<dyn Future<Item = <Self::Account as EthereumAccount>::AccountId, Error = ()> + Send>;

    /// Returns true if the transaction has already been processed and saved in
    /// the store.
    fn check_if_tx_processed(
        &self,
        tx_hash: H256,
    ) -> Box<dyn Future<Item = bool, Error = ()> + Send>;

    /// Saves the transaction hash in the store.
    fn mark_tx_processed(&self, tx_hash: H256) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}

/// Implement this trait for datatypes which can be used to sign an Ethereum
/// Transaction, e.g. an HSM, Ledger, Trezor connection, or a private key
/// string.
/// TODO: All methods should be converted to return a Future, since an HSM
/// connection is asynchronous
pub trait EthereumLedgerTxSigner {
    /// Takes a transaction and returns an RLP encoded signed version of it
    fn sign_raw_tx(&self, tx: RawTransaction, chain_id: u8) -> Vec<u8>;

    /// Takes a message and returns a signature on it
    fn sign_message(&self, message: &[u8]) -> Signature;

    /// Returns the Ethereum address associated with the signer
    fn address(&self) -> Address;
}

use secrecy::{Secret, ExposeSecret};

impl EthereumLedgerTxSigner for Secret<String> {
    fn sign_raw_tx(&self, tx: RawTransaction, chain_id: u8) -> Vec<u8> {
        tx.sign(&H256::from_str(self.expose_secret()).unwrap(), &chain_id)
    }

    fn sign_message(&self, message: &[u8]) -> Signature {
        let private_key: PrivateKey = self.expose_secret().parse().unwrap();
        let hash = Sha3::digest(message);
        private_key.sign_hash(&hash)
    }

    fn address(&self) -> Address {
        let private_key: PrivateKey = self.expose_secret().parse().unwrap();
        let address = private_key.to_public_key().unwrap();
        let mut out = [0; 20];
        out.copy_from_slice(address.as_bytes());
        // Address type from clarity library must convert to web3 Address
        Address::from(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_address() {
        let privkey =
            Secret::new(String::from("acb8f4184aaf6490b6e6aea7b474225be0d965eed75f4b91183eff6032c299f8"));
        let addr = privkey.address();
        assert_eq!(
            addr,
            Address::from_str("4070abbd2e38a8d27cd5a495f482c13f049f8310").unwrap()
        );
    }

    #[test]
    fn test_address_from_parity_ethkey() {
        let privkey =
            Secret::new(String::from("2569cb99b0936649aba55237c4952fc6f4090e016e8449a1d2f7cde142cfbb00"));
        let addr = privkey.address();
        assert_eq!(
            addr,
            Address::from_str("e78cf81c309f27d5c509114471dcd7c0f9de05fa").unwrap()
        );
    }
}
