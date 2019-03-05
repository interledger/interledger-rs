use super::{Account, AccountBuilder};
use bytes::Bytes;
use futures::{
    future::{err, ok},
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
    btp_auth: Arc<HashMap<(String, Option<String>), u64>>,
    http_auth: Arc<HashMap<String, u64>>,
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
        let btp_auth = Arc::new(HashMap::from_iter(accounts.iter().filter_map(
            |(account_id, account)| {
                if let Some(ref token) = account.inner.btp_incoming_token {
                    Some((
                        (
                            token.to_string(),
                            account.inner.btp_incoming_username.clone(),
                        ),
                        *account_id,
                    ))
                } else {
                    None
                }
            },
        )));
        let http_auth = Arc::new(HashMap::from_iter(accounts.iter().filter_map(
            |(account_id, account)| {
                if let Some(ref auth) = account.inner.http_incoming_authorization {
                    Some((auth.to_string(), *account_id))
                } else {
                    None
                }
            },
        )));

        InMemoryStore {
            accounts,
            routing_table: Arc::new(routing_table),
            btp_auth,
            http_auth,
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
        if let Some(account_id) = self.http_auth.get(auth_header) {
            Box::new(ok(self.accounts[account_id].clone()))
        } else {
            Box::new(err(()))
        }
    }
}

impl RouterStore for InMemoryStore {
    fn routing_table(&self) -> Vec<(Bytes, u64)> {
        self.routing_table.to_vec()
    }
}

impl BtpStore for InMemoryStore {
    type Account = Account;

    fn get_account_from_auth(
        &self,
        token: &str,
        username: Option<&str>,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send> {
        if let Some(account_id) = self
            .btp_auth
            .get(&(token.to_string(), username.map(|s| s.to_string())))
        {
            Box::new(ok(self.accounts[account_id].clone()))
        } else {
            Box::new(err(()))
        }
    }
}
