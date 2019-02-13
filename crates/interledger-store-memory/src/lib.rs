extern crate bytes;
extern crate futures;
extern crate hashbrown;
extern crate interledger_http;
extern crate interledger_service;
extern crate parking_lot;

use bytes::Bytes;
use futures::{future::result, Future};
use hashbrown::HashMap;
use interledger_http::store::{HttpDetails, HttpStore};
use interledger_service::AccountId;
use parking_lot::RwLock;
use std::sync::Arc;

pub struct Account {
    ilp_address: Bytes,
    asset_code: String,
    asset_scale: u8,
    http_endpoint: Option<String>,
    http_incoming_authorization: Option<String>,
    http_outgoing_authorization: Option<String>,
}

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
    ) -> Box<Future<Item = AccountId, Error = ()>> {
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

    fn get_http_details_for_account<'a>(
        &self,
        account_id: AccountId,
    ) -> Box<Future<Item = HttpDetails, Error = ()>> {
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
