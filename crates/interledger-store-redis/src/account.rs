use bytes::Bytes;
use interledger_api::{AccountDetails, NodeAccount};
use interledger_btp::BtpAccount;
use interledger_ccp::{CcpRoutingAccount, RoutingRelation};
use interledger_http::HttpAccount;
use interledger_ildcp::IldcpAccount;
use interledger_service::Account as AccountTrait;
use interledger_service_util::{
    MaxPacketAmountAccount, RoundTripTimeAccount, DEFAULT_ROUND_TRIP_TIME,
};
use redis::{from_redis_value, ErrorKind, FromRedisValue, RedisError, ToRedisArgs, Value};
use serde::Serializer;
use std::{
    collections::HashMap,
    str::{self, FromStr},
};
use url::Url;

const ACCOUNT_DETAILS_FIELDS: usize = 19;

#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub(crate) id: u64,
    #[serde(serialize_with = "address_to_string")]
    pub(crate) ilp_address: Bytes,
    // TODO add additional routes
    pub(crate) asset_code: String,
    pub(crate) asset_scale: u8,
    pub(crate) max_packet_amount: u64,
    pub(crate) min_balance: i64,
    #[serde(serialize_with = "optional_url_to_string")]
    pub(crate) http_endpoint: Option<Url>,
    pub(crate) http_incoming_authorization: Option<String>,
    pub(crate) http_outgoing_authorization: Option<String>,
    #[serde(serialize_with = "optional_url_to_string")]
    pub(crate) btp_uri: Option<Url>,
    pub(crate) btp_incoming_authorization: Option<String>,
    pub(crate) is_admin: bool,
    // TODO maybe take these out of the Account and insert them separately into the db
    // since they're only meant for the settlement engine
    pub(crate) xrp_address: Option<String>,
    pub(crate) settle_threshold: Option<i64>,
    pub(crate) settle_to: Option<i64>,
    #[serde(serialize_with = "routing_relation_to_string")]
    pub(crate) routing_relation: RoutingRelation,
    pub(crate) send_routes: bool,
    pub(crate) receive_routes: bool,
    pub(crate) round_trip_time: u64,
}

fn address_to_string<S>(address: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(str::from_utf8(address.as_ref()).unwrap_or(""))
}

fn optional_url_to_string<S>(url: &Option<Url>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if let Some(ref url) = url {
        serializer.serialize_str(url.as_ref())
    } else {
        serializer.serialize_none()
    }
}

fn routing_relation_to_string<S>(
    relation: &RoutingRelation,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(relation.to_string().as_str())
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
        let routing_relation = if let Some(ref relation) = details.routing_relation {
            RoutingRelation::from_str(relation)?
        } else {
            RoutingRelation::Child
        };
        Ok(Account {
            id,
            ilp_address: Bytes::from(details.ilp_address),
            asset_code: details.asset_code.to_uppercase(),
            asset_scale: details.asset_scale,
            max_packet_amount: details.max_packet_amount,
            min_balance: details.min_balance,
            http_endpoint,
            http_incoming_authorization: details.http_incoming_authorization,
            http_outgoing_authorization: details.http_outgoing_authorization,
            btp_uri,
            btp_incoming_authorization: details.btp_incoming_authorization,
            is_admin: details.is_admin,
            xrp_address: details.xrp_address,
            settle_threshold: details.settle_threshold,
            settle_to: details.settle_to,
            send_routes: details.send_routes,
            receive_routes: details.receive_routes,
            routing_relation,
            round_trip_time: details.round_trip_time.unwrap_or(DEFAULT_ROUND_TRIP_TIME),
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
        "routing_relation".write_redis_args(&mut rv);
        self.routing_relation.to_string().write_redis_args(&mut rv);
        "min_balance".write_redis_args(&mut rv);
        self.min_balance.write_redis_args(&mut rv);
        "round_trip_time".write_redis_args(&mut rv);
        self.round_trip_time.write_redis_args(&mut rv);

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
        if let Some(xrp_address) = self.xrp_address.as_ref() {
            "xrp_address".write_redis_args(&mut rv);
            xrp_address.write_redis_args(&mut rv);
        }
        if let Some(settle_threshold) = self.settle_threshold {
            "settle_threshold".write_redis_args(&mut rv);
            settle_threshold.write_redis_args(&mut rv);
        }
        if let Some(settle_to) = self.settle_to {
            "settle_to".write_redis_args(&mut rv);
            settle_to.write_redis_args(&mut rv);
        }
        if self.send_routes {
            "send_routes".write_redis_args(&mut rv);
            self.send_routes.write_redis_args(&mut rv);
        }
        if self.receive_routes {
            "receive_routes".write_redis_args(&mut rv);
            self.receive_routes.write_redis_args(&mut rv);
        }

        debug_assert!(rv.len() <= ACCOUNT_DETAILS_FIELDS * 2);
        debug_assert!((rv.len() % 2) == 0);

        ToRedisArgs::make_arg_vec(&rv, out);
    }
}

impl FromRedisValue for Account {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        let hash: HashMap<String, Value> = HashMap::from_redis_value(v)?;
        let ilp_address: String = get_value("ilp_address", &hash)?;
        let routing_relation: Option<String> = get_value_option("routing_relation", &hash)?;
        let routing_relation = if let Some(relation) = routing_relation {
            RoutingRelation::from_str(relation.as_str())
                .map_err(|_| RedisError::from((ErrorKind::TypeError, "Invalid Routing Relation")))?
        } else {
            RoutingRelation::Child
        };
        let round_trip_time: Option<u64> = get_value_option("round_trip_time", &hash)?;
        let round_trip_time: u64 = round_trip_time.unwrap_or(DEFAULT_ROUND_TRIP_TIME);
        Ok(Account {
            id: get_value("id", &hash)?,
            ilp_address: Bytes::from(ilp_address.as_bytes()),
            asset_code: get_value("asset_code", &hash)?,
            asset_scale: get_value("asset_scale", &hash)?,
            http_endpoint: get_url_option("http_endpoint", &hash)?,
            http_incoming_authorization: get_value_option("http_incoming_authorization", &hash)?,
            http_outgoing_authorization: get_value_option("http_outgoing_authorization", &hash)?,
            btp_uri: get_url_option("btp_uri", &hash)?,
            btp_incoming_authorization: get_value_option("btp_incoming_authorization", &hash)?,
            max_packet_amount: get_value("max_packet_amount", &hash)?,
            min_balance: get_value("min_balance", &hash)?,
            is_admin: get_bool("is_admin", &hash),
            xrp_address: get_value_option("xrp_address", &hash)?,
            settle_threshold: get_value_option("settle_threshold", &hash)?,
            settle_to: get_value_option("settle_to", &hash)?,
            routing_relation,
            send_routes: get_bool("send_routes", &hash),
            receive_routes: get_bool("receive_routes", &hash),
            round_trip_time,
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

fn get_bool(key: &str, map: &HashMap<String, Value>) -> bool {
    if let Some(ref value) = map.get(key) {
        if let Ok(value) = from_redis_value(value) as Result<String, RedisError> {
            if value.to_lowercase() == "true" {
                return true;
            }
        }
    }
    false
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

impl CcpRoutingAccount for Account {
    fn routing_relation(&self) -> RoutingRelation {
        self.routing_relation
    }

    fn should_send_routes(&self) -> bool {
        self.send_routes
    }

    fn should_receive_routes(&self) -> bool {
        self.receive_routes
    }
}

impl RoundTripTimeAccount for Account {
    fn round_trip_time(&self) -> u64 {
        self.round_trip_time
    }
}
