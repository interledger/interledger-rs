use bytes::Bytes;
use futures::{
    future::{err, ok, result},
    Future,
};
use hashbrown::HashMap;
use interledger_btp::{BtpAccount, BtpStore};
use interledger_http::{HttpAccount, HttpStore};
use interledger_ildcp::IldcpAccount;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore};
use interledger_service_util::MaxPacketAmountAccount;
use std::{
    fmt,
    iter::{once, FromIterator, IntoIterator},
    str,
    sync::Arc,
};
use url::Url;

pub struct AccountBuilder {
    pub id: u64,
    pub ilp_address: Bytes,
    pub additional_routes: Vec<Bytes>,
    pub asset_code: String,
    pub asset_scale: u8,
    pub http_endpoint: Option<Url>,
    pub http_incoming_authorization: Option<String>,
    pub http_outgoing_authorization: Option<String>,
    pub btp_url: Option<Url>,
    pub btp_incoming_authorization: Option<String>,
    pub max_packet_amount: u64,
}

impl AccountBuilder {
    pub fn build(self) -> Account {
        Account {
            inner: Arc::new(self),
        }
    }
}

// TODO should debugging print all the details or only the id and maybe ilp_address?
#[derive(Clone)]
pub struct Account {
    inner: Arc<AccountBuilder>,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Account {{ id: {}, ilp_address: {} }}",
            self.inner.id,
            str::from_utf8(&self.inner.ilp_address[..]).map_err(|_| fmt::Error)?,
        )
    }
}

impl AccountTrait for Account {
    type AccountId = u64;

    fn id(&self) -> Self::AccountId {
        self.inner.id
    }
}

impl IldcpAccount for Account {
    fn client_address(&self) -> Vec<u8> {
        self.inner.ilp_address.to_vec()
    }

    fn asset_code(&self) -> String {
        self.inner.asset_code.clone()
    }

    fn asset_scale(&self) -> u8 {
        self.inner.asset_scale
    }
}

impl MaxPacketAmountAccount for Account {
    fn max_packet_amount(&self) -> u64 {
        self.inner.max_packet_amount
    }
}

impl HttpAccount for Account {
    fn get_http_url(&self) -> Option<&Url> {
        self.inner.http_endpoint.as_ref()
    }

    fn get_http_auth_header(&self) -> Option<&str> {
        self.inner
            .http_outgoing_authorization
            .as_ref()
            .map(|s| s.as_str())
    }
}

impl BtpAccount for Account {
    fn get_btp_url(&self) -> Option<&Url> {
        self.inner.btp_url.as_ref()
    }
}

#[derive(Clone)]
pub struct InMemoryStore {
    accounts: Arc<HashMap<u64, Account>>,
}

impl InMemoryStore {
    pub fn new(accounts: impl IntoIterator<Item = AccountBuilder>) -> Self {
        let accounts = Arc::new(HashMap::from_iter(
            accounts
                .into_iter()
                .map(|builder| (builder.id, builder.build())),
        ));
        InMemoryStore { accounts }
    }

    pub fn from_accounts(accounts: impl IntoIterator<Item = Account>) -> Self {
        let accounts = Arc::new(HashMap::from_iter(
            accounts.into_iter().map(|account| (account.id(), account)),
        ));
        InMemoryStore { accounts }
    }
}

impl AccountStore for InMemoryStore {
    type Account = Account;

    fn get_account(&self, account_id: u64) -> Box<Future<Item = Account, Error = ()> + Send> {
        Box::new(result(
            self.accounts.get(&account_id).ok_or(()).map(|a| a.clone()),
        ))
    }

    fn get_accounts(
        &self,
        accounts_ids: &[u64],
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
    type Account = Account;

    fn routing_table<'a>(&'a self) -> Box<Iterator<Item = (&'a [u8], &'a Self::Account)> + 'a> {
        Box::new(self.accounts.values().flat_map(|account| {
            once((&account.inner.ilp_address[..], account)).chain(
                account
                    .inner
                    .additional_routes
                    .iter()
                    .map(move |ref route| (&route[..], account)),
            )
        }))
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
