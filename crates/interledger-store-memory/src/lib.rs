extern crate bytes;
extern crate futures;
extern crate hashbrown;
extern crate interledger_http;
extern crate interledger_router;
extern crate interledger_service;

use bytes::Bytes;
use futures::{
  future::{err, ok, result},
  Future,
};
use hashbrown::HashMap;
use interledger_http::{HttpDetails, HttpStore};
use interledger_router::RouterStore;
use interledger_service::AccountId;
use std::iter::once;
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
  accounts: Arc<HashMap<u64, Account>>,
  routing_table: Arc<Vec<(&'static [u8], (AccountId, &'static [u8]))>>,
}

impl InMemoryStore {
  pub fn new(accounts: HashMap<u64, Account>) -> Self {
    let accounts = Arc::new(accounts);
    let routing_table = Vec::with_capacity(accounts.iter().sum());
    for (account_id, account) in accounts.iter() {
      routing_table.push()
    }
    let routing_table = accounts
      .clone()
      .iter()
      .flat_map(|(account_id, account)| {
        once(account.ilp_address)
          .chain(account.additional_routes.into_iter())
          .map(move |address| (&address[..], (*account_id, &account.ilp_address[..])))
      })
      .collect();
    Self {
      accounts,
      routing_table: Arc::new(routing_table),
    }
  }
}

impl HttpStore for InMemoryStore {
  fn get_account_from_authorization(
    &self,
    auth_header: &str,
  ) -> Box<Future<Item = AccountId, Error = ()> + Send> {
    Box::new(result(
      self
        .accounts
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
    Box::new(result(self.accounts.get(&account_id).ok_or(()).and_then(
      |account| {
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
      },
    )))
  }
}

impl RouterStore for InMemoryStore {
  fn get_routing_table(&self) -> Arc<Vec<(&[u8], (AccountId, &[u8]))>> {
    self.routing_table.clone()
  }
}
