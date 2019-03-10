use bytes::Bytes;
use interledger_btp::BtpAccount;
use interledger_http::HttpAccount;
use interledger_ildcp::IldcpAccount;
use interledger_service::Account as AccountTrait;
use interledger_service_util::MaxPacketAmountAccount;
use std::{fmt, str, sync::Arc};
use url::Url;

/// A helper to create Accounts.
#[derive(Default)]
pub struct AccountBuilder {
    details: AccountDetails,
}

impl AccountBuilder {
    pub fn new() -> Self {
        AccountBuilder {
            details: AccountDetails::default(),
        }
    }

    pub fn build(self) -> Account {
        self.details.build()
    }

    pub fn id(mut self, id: u64) -> Self {
        self.details.id = id;
        self
    }

    pub fn ilp_address(mut self, ilp_address: &[u8]) -> Self {
        self.details.ilp_address = Bytes::from(ilp_address);
        self
    }

    pub fn additional_routes(mut self, routes: &[&[u8]]) -> Self {
        self.details.additional_routes = routes.iter().map(|route| Bytes::from(*route)).collect();
        self
    }

    pub fn asset_code(mut self, asset_code: String) -> Self {
        self.details.asset_code = asset_code;
        self
    }

    pub fn asset_scale(mut self, asset_scale: u8) -> Self {
        self.details.asset_scale = asset_scale;
        self
    }

    pub fn http_endpoint(mut self, http_endpoint: Url) -> Self {
        self.details.http_endpoint = Some(http_endpoint);
        self
    }

    pub fn http_incoming_authorization(mut self, auth_header: String) -> Self {
        self.details.http_incoming_authorization = Some(auth_header);
        self
    }

    pub fn http_outgoing_authorization(mut self, auth_header: String) -> Self {
        self.details.http_outgoing_authorization = Some(auth_header);
        self
    }

    pub fn btp_uri(mut self, uri: Url) -> Self {
        self.details.btp_uri = Some(uri);
        self
    }

    pub fn btp_incoming_token(mut self, auth_token: String) -> Self {
        self.details.btp_incoming_token = Some(auth_token);
        self
    }

    pub fn btp_incoming_username(mut self, username: Option<String>) -> Self {
        self.details.btp_incoming_username = username;
        self
    }

    pub fn max_packet_amount(mut self, amount: u64) -> Self {
        self.details.max_packet_amount = amount;
        self
    }
}

#[derive(Default, Clone)]
pub(crate) struct AccountDetails {
    pub(crate) id: u64,
    pub(crate) ilp_address: Bytes,
    pub(crate) additional_routes: Vec<Bytes>,
    pub(crate) asset_code: String,
    pub(crate) asset_scale: u8,
    pub(crate) http_endpoint: Option<Url>,
    pub(crate) http_incoming_authorization: Option<String>,
    pub(crate) http_outgoing_authorization: Option<String>,
    pub(crate) btp_uri: Option<Url>,
    pub(crate) btp_incoming_token: Option<String>,
    pub(crate) btp_incoming_username: Option<String>,
    pub(crate) max_packet_amount: u64,
}

impl AccountDetails {
    pub(crate) fn build(self) -> Account {
        Account {
            inner: Arc::new(self),
        }
    }
}

/// The Account type loaded from the InMemoryStore.
// TODO should debugging print all the details or only the id and maybe ilp_address?
#[derive(Clone)]
pub struct Account {
    pub(crate) inner: Arc<AccountDetails>,
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Account {{ id: {}, ilp_address: \"{}\" }}",
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
    fn client_address(&self) -> Bytes {
        self.inner.ilp_address.clone()
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
    fn get_btp_uri(&self) -> Option<&Url> {
        self.inner.btp_uri.as_ref()
    }
}
