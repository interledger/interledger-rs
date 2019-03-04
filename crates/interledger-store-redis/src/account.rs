use bytes::Bytes;
use interledger_btp::BtpAccount;
use interledger_http::HttpAccount;
use interledger_ildcp::IldcpAccount;
use interledger_service::Account as AccountTrait;
use interledger_service_util::MaxPacketAmountAccount;
use redis::{from_redis_value, ErrorKind, FromRedisValue, RedisError, ToRedisArgs, Value};
use std::sync::Arc;
use url::Url;

#[derive(Debug)]
pub struct AccountDetails {
    pub id: u64,
    pub ilp_address: Bytes,
    pub asset_code: String,
    pub asset_scale: u8,
    pub http_endpoint: Option<Url>,
    pub http_incoming_authorization: Option<String>,
    pub http_outgoing_authorization: Option<String>,
    pub btp_uri: Option<Url>,
    pub btp_incoming_authorization: Option<String>,
    pub max_packet_amount: u64,
}

impl Into<Account> for AccountDetails {
    fn into(self) -> Account {
        Account {
            inner: Arc::new(self),
        }
    }
}

impl ToRedisArgs for AccountDetails {
    fn write_redis_args(&self, out: &mut Vec<Vec<u8>>) {
        let tuple = (
            self.id,
            self.ilp_address.to_vec(),
            self.asset_code.clone(),
            self.asset_scale,
            self.http_endpoint.clone().map(|ref u| u.to_string()),
            self.http_incoming_authorization.clone(),
            self.http_outgoing_authorization.clone(),
            self.btp_uri.clone().map(|ref u| u.to_string()),
            self.btp_incoming_authorization.clone(),
            self.max_packet_amount,
        );
        tuple.write_redis_args(out);
    }
}

type AccountDetailsTuple = (
    u64,
    Vec<u8>,
    String,
    u8,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    u64,
);

impl FromRedisValue for AccountDetails {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        let (
            id,
            ilp_address,
            asset_code,
            asset_scale,
            http_endpoint,
            http_incoming_authorization,
            http_outgoing_authorization,
            btp_uri,
            btp_incoming_authorization,
            max_packet_amount,
        ): AccountDetailsTuple = from_redis_value(v)?;
        let ilp_address = Bytes::from(ilp_address);
        let http_endpoint = if let Some(s) = http_endpoint {
            Some(Url::parse(&s).map_err(|_err| {
                RedisError::from((ErrorKind::TypeError, "Unable to parse http_endpoint as URL"))
            })?)
        } else {
            None
        };
        let btp_uri = if let Some(s) = btp_uri {
            Some(Url::parse(&s).map_err(|_err| {
                RedisError::from((ErrorKind::TypeError, "Unable to parse btp_uri as URL"))
            })?)
        } else {
            None
        };

        Ok(AccountDetails {
            id,
            ilp_address,
            asset_code,
            asset_scale,
            http_endpoint,
            http_incoming_authorization,
            http_outgoing_authorization,
            btp_uri,
            btp_incoming_authorization,
            max_packet_amount,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Account {
    inner: Arc<AccountDetails>,
}

impl ToRedisArgs for Account {
    fn write_redis_args(&self, out: &mut Vec<Vec<u8>>) {
        self.inner.write_redis_args(out)
    }
}

impl FromRedisValue for Account {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        Ok(Account {
            inner: Arc::new(AccountDetails::from_redis_value(v)?),
        })
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

impl MaxPacketAmountAccount for Account {
    fn max_packet_amount(&self) -> u64 {
        self.inner.max_packet_amount
    }
}
