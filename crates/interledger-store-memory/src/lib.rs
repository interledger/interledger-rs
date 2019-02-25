use bytes::Bytes;
use futures::{future::result, Future};
use hashbrown::HashMap;
use interledger_btp::BtpStore;
use interledger_http::{HttpDetails, HttpStore};
use interledger_ildcp::{AccountDetails, IldcpStore};
use interledger_router::RouterStore;
use interledger_service::AccountId;
use interledger_service_util::MaxPacketAmountStore;
use std::sync::Arc;
use url::Url;

pub struct Account {
  ilp_address: Bytes,
  additional_routes: Vec<Bytes>,
  asset_code: String,
  asset_scale: u8,
  http_endpoint: Option<String>,
  http_incoming_authorization: Option<String>,
  http_outgoing_authorization: Option<String>,
  btp_url: Option<Url>,
  btp_incoming_authorization: Option<String>,
  max_packet_amount: u64,
}

#[derive(Clone)]
pub struct InMemoryStore {
  accounts: Arc<HashMap<u64, Account>>,
  routing_table: Arc<Vec<(Bytes, AccountId)>>,
}

impl InMemoryStore {
  pub fn new(accounts: HashMap<u64, Account>) -> Self {
    let accounts = Arc::new(accounts);
    let mut routing_table = Vec::with_capacity(
      accounts
        .values()
        .map(|account| 1usize + account.additional_routes.len())
        .sum(),
    );
    for (account_id, account) in accounts.iter() {
      routing_table.push((account.ilp_address.clone(), *account_id));

      for route in account.additional_routes.iter() {
        routing_table.push((route.clone(), *account_id));
      }
    }
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
  fn get_routing_table(&self) -> Arc<Vec<(Bytes, AccountId)>> {
    self.routing_table.clone()
  }
}

impl IldcpStore for InMemoryStore {
  fn get_account_details(
    &self,
    account_id: AccountId,
  ) -> Box<Future<Item = AccountDetails, Error = ()> + Send> {
    Box::new(result(
      self
        .accounts
        .get(&account_id)
        .map(|account| AccountDetails {
          client_address: account.ilp_address.clone(),
          asset_code: account.asset_code.clone(),
          asset_scale: account.asset_scale,
        })
        .ok_or(()),
    ))
  }
}

impl MaxPacketAmountStore for InMemoryStore {
  fn get_max_packet_amount(
    &self,
    account_id: AccountId,
  ) -> Box<Future<Item = u64, Error = ()> + Send> {
    Box::new(result(
      self
        .accounts
        .get(&account_id)
        .map(|account| account.max_packet_amount)
        .ok_or(()),
    ))
  }
}

impl BtpStore for InMemoryStore {
  fn get_account_from_token(
    &self,
    token: &str,
  ) -> Box<Future<Item = AccountId, Error = ()> + Send> {
    Box::new(result(
      self
        .accounts
        .iter()
        .find(|(_account_id, account)| {
          if let Some(auth) = &account.btp_incoming_authorization {
            token == auth.as_str()
          } else {
            false
          }
        })
        .map(|(account_id, _account)| *account_id)
        .ok_or(()),
    ))
  }

  fn get_btp_url(&self, account: &AccountId) -> Box<Future<Item = Url, Error = ()> + Send> {
    Box::new(result(self.accounts.get(account).ok_or(()).and_then(
      |account| {
        if let Some(ref url) = account.btp_url {
          Ok(url.clone())
        } else {
          Err(())
        }
      },
    )))
  }
}
