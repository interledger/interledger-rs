use bytes::Bytes;
use interledger_api::{AccountDetails, NodeAccount};
use interledger_btp::BtpAccount;
use interledger_http::HttpAccount;
use interledger_ildcp::IldcpAccount;
use interledger_service::Account as AccountTrait;
use interledger_service_util::MaxPacketAmountAccount;
use redis::{from_redis_value, ErrorKind, FromRedisValue, RedisError, ToRedisArgs, Value};
use std::collections::HashMap;
use url::Url;

const ACCOUNT_DETAILS_FIELDS: usize = 11;

#[derive(Clone, Debug)]
pub struct Account {
    pub(crate) id: u64,
    pub(crate) ilp_address: Bytes,
    pub(crate) asset_code: String,
    pub(crate) asset_scale: u8,
    pub(crate) max_packet_amount: u64,
    pub(crate) http_endpoint: Option<Url>,
    pub(crate) http_incoming_authorization: Option<String>,
    pub(crate) http_outgoing_authorization: Option<String>,
    pub(crate) btp_uri: Option<Url>,
    pub(crate) btp_incoming_authorization: Option<String>,
    pub(crate) is_admin: bool,
}

impl Account {
    pub fn try_from(id: u64, details: AccountDetails) -> Result<Account, ()> {
        let http_endpoint = if let Some(ref url) = details.http_endpoint {
            Some(Url::parse(url).map_err(|err| error!("Invalid URL: {:?}", err))?)
        } else {
            None
        };
        let btp_uri = if let Some(ref url) = details.btp_uri {
            Some(Url::parse(url).map_err(|err| error!("Invalid URL: {:?}", err))?)
        } else {
            None
        };
        Ok(Account {
            id,
            ilp_address: Bytes::from(details.ilp_address),
            asset_code: details.asset_code,
            asset_scale: details.asset_scale,
            max_packet_amount: details.max_packet_amount,
            http_endpoint,
            http_incoming_authorization: details.http_incoming_authorization,
            http_outgoing_authorization: details.http_outgoing_authorization,
            btp_uri,
            btp_incoming_authorization: details.btp_incoming_authorization,
            is_admin: details.is_admin,
        })
    }
}

impl ToRedisArgs for Account {
    fn write_redis_args(&self, out: &mut Vec<Vec<u8>>) {
        let mut rv = Vec::with_capacity(ACCOUNT_DETAILS_FIELDS * 2);

        "id".write_redis_args(&mut rv);
        self.id.write_redis_args(&mut rv);
        if !self.ilp_address.is_empty() {
            "ilp_address".write_redis_args(&mut rv);
            rv.push(self.ilp_address.to_vec());
        }
        if !self.asset_code.is_empty() {
            "asset_code".write_redis_args(&mut rv);
            self.asset_code.write_redis_args(&mut rv);
        }
        "asset_scale".write_redis_args(&mut rv);
        self.asset_scale.write_redis_args(&mut rv);
        "max_packet_amount".write_redis_args(&mut rv);
        self.max_packet_amount.write_redis_args(&mut rv);
        "is_admin".write_redis_args(&mut rv);
        self.is_admin.write_redis_args(&mut rv);

        // Write optional fields
        if let Some(http_endpoint) = self.http_endpoint.as_ref() {
            "http_endpoint".write_redis_args(&mut rv);
            http_endpoint.as_str().write_redis_args(&mut rv);
        }
        if let Some(http_incoming_authorization) = self.http_incoming_authorization.as_ref() {
            "http_incoming_authorization".write_redis_args(&mut rv);
            http_incoming_authorization.write_redis_args(&mut rv);
        }
        if let Some(http_outgoing_authorization) = self.http_outgoing_authorization.as_ref() {
            "http_outgoing_authorization".write_redis_args(&mut rv);
            http_outgoing_authorization.write_redis_args(&mut rv);
        }
        if let Some(btp_uri) = self.btp_uri.as_ref() {
            "btp_uri".write_redis_args(&mut rv);
            btp_uri.as_str().write_redis_args(&mut rv);
        }
        if let Some(btp_incoming_authorization) = self.btp_incoming_authorization.as_ref() {
            "btp_incoming_authorization".write_redis_args(&mut rv);
            btp_incoming_authorization.write_redis_args(&mut rv);
        }

        debug_assert!(rv.len() < ACCOUNT_DETAILS_FIELDS * 2);
        debug_assert!((rv.len() % 2) == 0);

        ToRedisArgs::make_arg_vec(&rv, out);
    }
}

impl FromRedisValue for Account {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        trace!("Loaded value from Redis: {:?}", v);
        let hash: HashMap<String, Value> = HashMap::from_redis_value(v)?;
        let ilp_address: Vec<u8> = get_value("ilp_address", &hash)?;
        let is_admin: String = get_value("is_admin", &hash)?;
        let is_admin = is_admin == "true";
        Ok(Account {
            id: get_value("id", &hash)?,
            ilp_address: Bytes::from(ilp_address),
            asset_code: get_value("asset_code", &hash)?,
            asset_scale: get_value("asset_scale", &hash)?,
            http_endpoint: get_url_option("http_endpoint", &hash)?,
            http_incoming_authorization: get_value_option("http_incoming_authorization", &hash)?,
            http_outgoing_authorization: get_value_option("http_outgoing_authorization", &hash)?,
            btp_uri: get_url_option("btp_uri", &hash)?,
            btp_incoming_authorization: get_value_option("btp_incoming_authorization", &hash)?,
            max_packet_amount: get_value("max_packet_amount", &hash)?,
            is_admin,
        })
    }
}

fn get_value<V>(key: &str, map: &HashMap<String, Value>) -> Result<V, RedisError>
where
    V: FromRedisValue,
{
    if let Some(ref value) = map.get(key) {
        from_redis_value(value)
    } else {
        Err(RedisError::from((
            ErrorKind::TypeError,
            "Account is missing field",
            key.to_string(),
        )))
    }
}

fn get_value_option<V>(key: &str, map: &HashMap<String, Value>) -> Result<Option<V>, RedisError>
where
    V: FromRedisValue,
{
    if let Some(ref value) = map.get(key) {
        from_redis_value(value).map(Some)
    } else {
        Ok(None)
    }
}

fn get_url_option(key: &str, map: &HashMap<String, Value>) -> Result<Option<Url>, RedisError> {
    if let Some(ref value) = map.get(key) {
        let value: String = from_redis_value(value)?;
        if let Ok(url) = Url::parse(&value) {
            Ok(Some(url))
        } else {
            Err(RedisError::from((ErrorKind::TypeError, "Invalid URL")))
        }
    } else {
        Ok(None)
    }
}

impl AccountTrait for Account {
    type AccountId = u64;

    fn id(&self) -> Self::AccountId {
        self.id
    }
}

impl IldcpAccount for Account {
    fn client_address(&self) -> &[u8] {
        self.ilp_address.as_ref()
    }

    fn asset_code(&self) -> &str {
        self.asset_code.as_str()
    }

    fn asset_scale(&self) -> u8 {
        self.asset_scale
    }
}

impl HttpAccount for Account {
    fn get_http_url(&self) -> Option<&Url> {
        self.http_endpoint.as_ref()
    }

    fn get_http_auth_header(&self) -> Option<&str> {
        self.http_outgoing_authorization
            .as_ref()
            .map(|s| s.as_str())
    }
}

impl BtpAccount for Account {
    fn get_btp_uri(&self) -> Option<&Url> {
        self.btp_uri.as_ref()
    }
}

impl MaxPacketAmountAccount for Account {
    fn max_packet_amount(&self) -> u64 {
        self.max_packet_amount
    }
}

impl NodeAccount for Account {
    fn is_admin(&self) -> bool {
        self.is_admin
    }
}
