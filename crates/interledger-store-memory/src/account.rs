use bytes::Bytes;
use interledger_btp::BtpAccount;
use interledger_http::HttpAccount;
use interledger_ildcp::IldcpAccount;
use interledger_service::Account as AccountTrait;
use interledger_service_util::MaxPacketAmountAccount;
use std::{fmt, str, sync::Arc};
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
    pub btp_uri: Option<Url>,
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
    pub(crate) inner: Arc<AccountBuilder>,
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
