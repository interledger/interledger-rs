use super::{Account, AccountBuilder};
use bytes::Bytes;
use futures::{
    future::{err, ok},
    Future,
};
use hashbrown::HashMap;
use interledger_btp::{BtpOpenSignupAccount, BtpOpenSignupStore, BtpStore};
use interledger_http::HttpStore;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore};
use parking_lot::{Mutex, RwLock};
use std::{
    cmp::max,
    iter::{empty, once, FromIterator, IntoIterator},
    str,
    sync::Arc,
};

/// A simple in-memory store intended primarily for testing and
/// stateless sender/receiver services that are passed all of the
/// relevant account details when the store is instantiated.
#[derive(Clone)]
pub struct InMemoryStore {
    accounts: Arc<RwLock<HashMap<u64, Account>>>,
    routing_table: Arc<RwLock<HashMap<Bytes, u64>>>,
    btp_auth: Arc<RwLock<HashMap<String, u64>>>,
    http_auth: Arc<RwLock<HashMap<String, u64>>>,
    next_account_id: Arc<Mutex<u64>>,
}

impl InMemoryStore {
    pub fn new(accounts: impl IntoIterator<Item = AccountBuilder>) -> Self {
        InMemoryStore::from_accounts(accounts.into_iter().map(|builder| builder.build()))
    }

    pub fn default() -> Self {
        InMemoryStore::from_accounts(empty())
    }

    pub fn from_accounts(accounts: impl IntoIterator<Item = Account>) -> Self {
        let mut next_account_id: u64 = 0;

        let accounts = HashMap::from_iter(accounts.into_iter().map(|account| {
            next_account_id = max(account.id(), next_account_id);
            (account.id(), account)
        }));
        next_account_id += 1;

        let routing_table: HashMap<Bytes, u64> =
            HashMap::from_iter(accounts.iter().flat_map(|(account_id, account)| {
                once((account.inner.ilp_address.clone(), *account_id)).chain(
                    account
                        .inner
                        .additional_routes
                        .iter()
                        .map(move |route| (route.clone(), *account_id)),
                )
            }));

        let btp_auth = HashMap::from_iter(accounts.iter().filter_map(|(account_id, account)| {
            if let Some(ref token) = account.inner.btp_incoming_token {
                Some((token.to_string(), *account_id))
            } else {
                None
            }
        }));
        let http_auth = HashMap::from_iter(accounts.iter().filter_map(|(account_id, account)| {
            if let Some(ref auth) = account.inner.http_incoming_token {
                Some((auth.to_string(), *account_id))
            } else {
                None
            }
        }));

        InMemoryStore {
            accounts: Arc::new(RwLock::new(accounts)),
            routing_table: Arc::new(RwLock::new(routing_table)),
            btp_auth: Arc::new(RwLock::new(btp_auth)),
            http_auth: Arc::new(RwLock::new(http_auth)),
            next_account_id: Arc::new(Mutex::new(next_account_id)),
        }
    }

    pub fn add_account(&self, account: Account) {
        self.accounts.write().insert(account.id(), account.clone());
        self.routing_table
            .write()
            .insert(account.inner.ilp_address.clone(), account.id());
        for route in &account.inner.additional_routes {
            self.routing_table
                .write()
                .insert(route.clone(), account.id());
        }
        if let Some(ref btp_auth) = account.inner.btp_incoming_token {
            self.btp_auth.write().insert(btp_auth.clone(), account.id());
        }
        if let Some(ref http_auth) = account.inner.http_incoming_token {
            self.http_auth
                .write()
                .insert(http_auth.clone(), account.id());
        }
        let mut next_account_id = self.next_account_id.lock();
        *next_account_id = max(*next_account_id, account.inner.id);
    }
}

impl AccountStore for InMemoryStore {
    type Account = Account;

    fn get_accounts(
        &self,
        accounts_ids: Vec<u64>,
    ) -> Box<Future<Item = Vec<Account>, Error = ()> + Send> {
        let accounts: Vec<Account> = accounts_ids
            .iter()
            .filter_map(|account_id| self.accounts.read().get(account_id).cloned())
            .collect();
        if accounts.len() == accounts_ids.len() {
            Box::new(ok(accounts))
        } else {
            Box::new(err(()))
        }
    }
}

impl HttpStore for InMemoryStore {
    type Account = Account;

    fn get_account_from_http_token(
        &self,
        auth_header: &str,
    ) -> Box<Future<Item = Account, Error = ()> + Send> {
        if let Some(account_id) = self.http_auth.read().get(auth_header) {
            if let Some(account) = self.accounts.read().get(account_id) {
                return Box::new(ok(account.clone()));
            }
        }
        Box::new(err(()))
    }
}

impl RouterStore for InMemoryStore {
    fn routing_table(&self) -> HashMap<Bytes, u64> {
        self.routing_table.read().clone()
    }
}

impl BtpStore for InMemoryStore {
    type Account = Account;

    fn get_account_from_btp_token(
        &self,
        token: &str,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send> {
        if let Some(account_id) = self.btp_auth.read().get(&(token.to_string())) {
            Box::new(ok(self.accounts.read()[account_id].clone()))
        } else {
            Box::new(err(()))
        }
    }

    fn get_btp_outgoing_accounts(
        &self,
    ) -> Box<Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        Box::new(ok(self
            .accounts
            .read()
            .values()
            .filter(|account| (**account).inner.btp_uri.is_some())
            .cloned()
            .collect()))
    }
}

impl BtpOpenSignupStore for InMemoryStore {
    type Account = Account;

    fn create_btp_account<'a>(
        &self,
        account: BtpOpenSignupAccount<'a>,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send> {
        let account_id = {
            let next_id: u64 = *self.next_account_id.lock();
            *self.next_account_id.lock() += 1;
            next_id
        };
        let account = AccountBuilder::new()
            .id(account_id)
            .ilp_address(account.ilp_address)
            .btp_incoming_token(account.auth_token.to_string())
            .asset_code(account.asset_code.to_string())
            .asset_scale(account.asset_scale)
            .build();

        (*self.accounts.write()).insert(account_id, account.clone());
        (*self.routing_table.write()).insert(account.inner.ilp_address.clone(), account_id);
        (*self.btp_auth.write()).insert(
            account.inner.btp_incoming_token.clone().unwrap(),
            account_id,
        );

        Box::new(ok(account))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_accounts() {
        let store = InMemoryStore::new(vec![
            AccountBuilder::new().id(0),
            AccountBuilder::new().id(1),
            AccountBuilder::new().id(4),
        ]);
        let accounts = store.get_accounts(vec![0, 4]).wait().unwrap();
        assert_eq!(accounts[0].id(), 0);
        assert_eq!(accounts[1].id(), 4);

        assert!(store.get_accounts(vec![0, 5]).wait().is_err());
    }

    #[test]
    fn query_by_http_auth() {
        let account = AccountBuilder::new()
            .http_incoming_token("test_token".to_string())
            .build();
        let store = InMemoryStore::from_accounts(vec![account]);
        store
            .get_account_from_http_token("test_token")
            .wait()
            .unwrap();
        assert!(store
            .get_account_from_http_token("bad_token")
            .wait()
            .is_err());
    }

    #[test]
    fn query_by_btp() {
        let account = AccountBuilder::new()
            .btp_incoming_token("test_token".to_string())
            .build();
        let store = InMemoryStore::from_accounts(vec![account]);
        store
            .get_account_from_btp_token("test_token")
            .wait()
            .unwrap();
        assert!(store
            .get_account_from_btp_token("bad_token")
            .wait()
            .is_err());
    }

    #[test]
    fn routing_table() {
        let store = InMemoryStore::new(vec![
            AccountBuilder::new()
                .id(1)
                .ilp_address(b"example.one")
                .additional_routes(&[b"example.three"]),
            AccountBuilder::new().id(2).ilp_address(b"example.two"),
        ]);

        assert_eq!(
            store.routing_table(),
            HashMap::from_iter(vec![
                (Bytes::from("example.one"), 1),
                (Bytes::from("example.two"), 2),
                (Bytes::from("example.three"), 1)
            ])
        );
    }

    #[test]
    fn open_btp_signup() {
        let store = InMemoryStore::default();
        let account = store
            .create_btp_account(BtpOpenSignupAccount {
                auth_token: "token",
                ilp_address: b"example.account",
                asset_code: "XYZ",
                asset_scale: 9,
            })
            .wait()
            .unwrap();
        assert_eq!(account.id(), 1);
    }
}
