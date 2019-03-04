use super::{Account, AccountBuilder};
use bytes::Bytes;
use futures::{
    future::{err, ok, result},
    Future,
};
use hashbrown::HashMap;
use interledger_btp::BtpStore;
use interledger_http::HttpStore;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore};
use std::{
    iter::{once, FromIterator, IntoIterator},
    str,
    sync::Arc,
};

#[derive(Clone)]
pub struct InMemoryStore {
    accounts: Arc<HashMap<u64, Account>>,
    routing_table: Arc<Vec<(Bytes, u64)>>,
}

impl InMemoryStore {
    pub fn new(accounts: impl IntoIterator<Item = AccountBuilder>) -> Self {
        InMemoryStore::from_accounts(accounts.into_iter().map(|builder| builder.build()))
    }

    pub fn from_accounts(accounts: impl IntoIterator<Item = Account>) -> Self {
        let accounts = Arc::new(HashMap::from_iter(
            accounts.into_iter().map(|account| (account.id(), account)),
        ));
        let routing_table: Vec<(Bytes, u64)> = accounts
            .iter()
            .flat_map(|(account_id, account)| {
                once((account.inner.ilp_address.clone(), *account_id)).chain(
                    account
                        .inner
                        .additional_routes
                        .iter()
                        .map(move |route| (route.clone(), *account_id)),
                )
            })
            .collect();
        InMemoryStore {
            accounts,
            routing_table: Arc::new(routing_table),
        }
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
            .filter_map(|account_id| self.accounts.get(account_id).cloned())
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

    // TODO this should use a hashmap internally
    fn get_account_from_authorization(
        &self,
        auth_header: &str,
    ) -> Box<Future<Item = Account, Error = ()> + Send> {
        Box::new(result(
            self.accounts
                .iter()
                .find(|(_account_id, account)| {
                    if let Some(ref header) = account.inner.http_incoming_authorization {
                        if header == auth_header {
                            return true;
                        }
                    }
                    false
                })
                .ok_or(())
                .map(|(_account_id, account)| account.clone()),
        ))
    }
}

impl RouterStore for InMemoryStore {
    fn routing_table(&self) -> Vec<(Bytes, u64)> {
        self.routing_table.to_vec()
    }
}

impl BtpStore for InMemoryStore {
    type Account = Account;

    fn get_account_from_token(
        &self,
        token: &str,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send> {
        Box::new(result(
            self.accounts
                .iter()
                .find(|(_account_id, account)| {
                    if let Some(auth) = &account.inner.btp_incoming_authorization {
                        token == auth.as_str()
                    } else {
                        false
                    }
                })
                .map(|(_account_id, account)| account.clone())
                .ok_or(()),
        ))
    }
}
