extern crate bytes;
extern crate futures;
extern crate hashbrown;
extern crate interledger_http;
extern crate interledger_router;
extern crate interledger_service;
extern crate parking_lot;

use bytes::Bytes;
use futures::{
    future::{err, ok, result},
    Future,
};
use hashbrown::HashMap;
use interledger_http::{HttpDetails, HttpStore};
use interledger_router::RouterStore;
use interledger_service::AccountId;
use parking_lot::RwLock;
use std::sync::Arc;

pub struct Account {
    ilp_address: Bytes,
    additional_routes: Vec<Bytes>,
    asset_code: String,
    asset_scale: u8,
    http_endpoint: Option<String>,
    http_incoming_authorization: Option<String>,
    http_outgoing_authorization: Option<String>,
}

#[derive(Clone)]
pub struct InMemoryStore {
    accounts: Arc<RwLock<HashMap<u64, Account>>>,
}

impl InMemoryStore {
    pub fn new(accounts: HashMap<u64, Account>) -> Self {
        Self {
            accounts: Arc::new(RwLock::new(accounts)),
        }
    }
}

impl HttpStore for InMemoryStore {
    fn get_account_from_authorization(
        &self,
        auth_header: &str,
    ) -> Box<Future<Item = AccountId, Error = ()> + Send> {
        Box::new(result(
            (*self.accounts.read())
                .iter()
                .find(|(_account_id, account)| {
                    if let Some(ref header) = account.http_incoming_authorization {
                        if header == auth_header {
                            return true;
                        }
                    }
                    false
                })
                .ok_or(())
                .map(|(account_id, _account)| *account_id),
        ))
    }

    fn get_http_details_for_account(
        &self,
        account_id: AccountId,
    ) -> Box<Future<Item = HttpDetails, Error = ()> + Send> {
        Box::new(result(
            (*self.accounts.read())
                .get(&account_id)
                .ok_or(())
                .and_then(|account| {
                    if let Some(url) = &account.http_endpoint {
                        let auth_header = if let Some(auth) = &account.http_outgoing_authorization {
                            auth.to_string()
                        } else {
                            String::new()
                        };
                        Ok(HttpDetails {
                            url: url.to_string(),
                            auth_header,
                        })
                    } else {
                        Err(())
                    }
                }),
        ))
    }
}

impl RouterStore for InMemoryStore {
    fn get_next_hop(
        &self,
        destination: &[u8],
    ) -> Box<Future<Item = (AccountId, Bytes), Error = ()> + Send> {
        let mut next_hop: Option<AccountId> = None;
        let mut max_prefix_len = 0;

        for (account_id, account) in self.accounts.read().iter() {
            if destination.starts_with(&account.ilp_address)
                && account.ilp_address.len() > max_prefix_len
            {
                next_hop = Some(*account_id);
                max_prefix_len = account.ilp_address.len();
            }

            for address in account.additional_routes.iter() {
                if destination.starts_with(&address) && address.len() > max_prefix_len {
                    next_hop = Some(*account_id);
                    max_prefix_len = address.len();
                }
            }
        }

        if let Some(account_id) = next_hop {
            Box::new(ok((
                account_id,
                self.accounts.read()[&account_id].ilp_address.clone(),
            )))
        } else {
            Box::new(err(()))
        }
    }
}
