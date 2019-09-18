use bytes::Bytes;
use interledger_ccp::RoutingRelation;
use interledger_packet::Address;
use interledger_service::Username;
use interledger_service_util::DEFAULT_ROUND_TRIP_TIME;

use std::{
    collections::HashMap,
    str::{self, FromStr},
};
use uuid::Uuid;
use url::Url;
const ACCOUNT_DETAILS_FIELDS: usize = 21;

use secrecy::ExposeSecret;
use secrecy::SecretBytes;

use redis_crate::{
    from_redis_value, ErrorKind, FromRedisValue, RedisError, RedisWrite, ToRedisArgs, Value,
};

use crate::utils::account::{Account, AccountId};
use crate::utils::encrypted_account::AccountWithEncryptedTokens;

impl ToRedisArgs for AccountId {
    fn write_redis_args<W: RedisWrite + ?Sized>(&self, out: &mut W) {
        out.write_arg(self.0.to_hyphenated().to_string().as_bytes().as_ref());
    }
}

impl FromRedisValue for AccountId {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        let account_id = String::from_redis_value(v)?;
        let uid = Uuid::from_str(&account_id)
            .map_err(|_| RedisError::from((ErrorKind::TypeError, "Invalid account id string")))?;
        Ok(AccountId(uid))
    }
}

impl ToRedisArgs for AccountWithEncryptedTokens {
    fn write_redis_args<W: RedisWrite + ?Sized>(&self, out: &mut W) {
        let mut rv = Vec::with_capacity(ACCOUNT_DETAILS_FIELDS * 2);
        let account = &self.account;

        "id".write_redis_args(&mut rv);
        account.id.write_redis_args(&mut rv);
        "username".write_redis_args(&mut rv);
        account
            .username
            .as_bytes()
            .to_vec()
            .write_redis_args(&mut rv);
        if !account.ilp_address.is_empty() {
            "ilp_address".write_redis_args(&mut rv);
            rv.push(account.ilp_address.to_bytes().to_vec());
        }
        if !account.asset_code.is_empty() {
            "asset_code".write_redis_args(&mut rv);
            account.asset_code.write_redis_args(&mut rv);
        }
        "asset_scale".write_redis_args(&mut rv);
        account.asset_scale.write_redis_args(&mut rv);
        "max_packet_amount".write_redis_args(&mut rv);
        account.max_packet_amount.write_redis_args(&mut rv);
        "routing_relation".write_redis_args(&mut rv);
        account
            .routing_relation
            .to_string()
            .write_redis_args(&mut rv);
        "round_trip_time".write_redis_args(&mut rv);
        account.round_trip_time.write_redis_args(&mut rv);

        // Write optional fields
        if let Some(http_endpoint) = account.http_endpoint.as_ref() {
            "http_endpoint".write_redis_args(&mut rv);
            http_endpoint.as_str().write_redis_args(&mut rv);
        }
        if let Some(http_incoming_token) = account.http_incoming_token.as_ref() {
            "http_incoming_token".write_redis_args(&mut rv);
            http_incoming_token
                .expose_secret()
                .as_ref()
                .write_redis_args(&mut rv);
        }
        if let Some(http_outgoing_token) = account.http_outgoing_token.as_ref() {
            "http_outgoing_token".write_redis_args(&mut rv);
            http_outgoing_token
                .expose_secret()
                .as_ref()
                .write_redis_args(&mut rv);
        }
        if let Some(btp_uri) = account.btp_uri.as_ref() {
            "btp_uri".write_redis_args(&mut rv);
            btp_uri.as_str().write_redis_args(&mut rv);
        }
        if let Some(btp_incoming_token) = account.btp_incoming_token.as_ref() {
            "btp_incoming_token".write_redis_args(&mut rv);
            btp_incoming_token
                .expose_secret()
                .as_ref()
                .write_redis_args(&mut rv);
        }
        if let Some(btp_outgoing_token) = account.btp_outgoing_token.as_ref() {
            "btp_outgoing_token".write_redis_args(&mut rv);
            btp_outgoing_token
                .expose_secret()
                .as_ref()
                .write_redis_args(&mut rv);
        }
        if let Some(settle_threshold) = account.settle_threshold {
            "settle_threshold".write_redis_args(&mut rv);
            settle_threshold.write_redis_args(&mut rv);
        }
        if let Some(settle_to) = account.settle_to {
            "settle_to".write_redis_args(&mut rv);
            settle_to.write_redis_args(&mut rv);
        }
        if let Some(limit) = account.packets_per_minute_limit {
            "packets_per_minute_limit".write_redis_args(&mut rv);
            limit.write_redis_args(&mut rv);
        }
        if let Some(limit) = account.amount_per_minute_limit {
            "amount_per_minute_limit".write_redis_args(&mut rv);
            limit.write_redis_args(&mut rv);
        }
        if let Some(min_balance) = account.min_balance {
            "min_balance".write_redis_args(&mut rv);
            min_balance.write_redis_args(&mut rv);
        }
        if let Some(settlement_engine_url) = &account.settlement_engine_url {
            "settlement_engine_url".write_redis_args(&mut rv);
            settlement_engine_url.as_str().write_redis_args(&mut rv);
        }

        debug_assert!(rv.len() <= ACCOUNT_DETAILS_FIELDS * 2);
        debug_assert!((rv.len() % 2) == 0);

        ToRedisArgs::make_arg_vec(&rv, out);
    }
}

impl FromRedisValue for AccountWithEncryptedTokens {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        let hash: HashMap<String, Value> = HashMap::from_redis_value(v)?;
        let ilp_address: String = get_value("ilp_address", &hash)?;
        let ilp_address = Address::from_str(&ilp_address)
            .map_err(|_| RedisError::from((ErrorKind::TypeError, "Invalid ILP address")))?;
        let username: String = get_value("username", &hash)?;
        let username = Username::from_str(&username)
            .map_err(|_| RedisError::from((ErrorKind::TypeError, "Invalid username")))?;
        let routing_relation: Option<String> = get_value_option("routing_relation", &hash)?;
        let routing_relation = if let Some(relation) = routing_relation {
            RoutingRelation::from_str(relation.as_str())
                .map_err(|_| RedisError::from((ErrorKind::TypeError, "Invalid Routing Relation")))?
        } else {
            RoutingRelation::Child
        };
        let round_trip_time: Option<u32> = get_value_option("round_trip_time", &hash)?;
        let round_trip_time: u32 = round_trip_time.unwrap_or(DEFAULT_ROUND_TRIP_TIME);

        Ok(AccountWithEncryptedTokens {
            account: Account {
                id: get_value("id", &hash)?,
                username,
                ilp_address,
                asset_code: get_value("asset_code", &hash)?,
                asset_scale: get_value("asset_scale", &hash)?,
                http_endpoint: get_url_option("http_endpoint", &hash)?,
                http_incoming_token: get_bytes_option("http_incoming_token", &hash)?
                    .map(SecretBytes::from),
                http_outgoing_token: get_bytes_option("http_outgoing_token", &hash)?
                    .map(SecretBytes::from),
                btp_uri: get_url_option("btp_uri", &hash)?,
                btp_incoming_token: get_bytes_option("btp_incoming_token", &hash)?
                    .map(SecretBytes::from),
                btp_outgoing_token: get_bytes_option("btp_outgoing_token", &hash)?
                    .map(SecretBytes::from),
                max_packet_amount: get_value("max_packet_amount", &hash)?,
                min_balance: get_value_option("min_balance", &hash)?,
                settle_threshold: get_value_option("settle_threshold", &hash)?,
                settle_to: get_value_option("settle_to", &hash)?,
                routing_relation,
                round_trip_time,
                packets_per_minute_limit: get_value_option("packets_per_minute_limit", &hash)?,
                amount_per_minute_limit: get_value_option("amount_per_minute_limit", &hash)?,
                settlement_engine_url: get_url_option("settlement_engine_url", &hash)?,
            },
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

fn get_bytes_option(key: &str, map: &HashMap<String, Value>) -> Result<Option<Bytes>, RedisError> {
    if let Some(ref value) = map.get(key) {
        let vec: Vec<u8> = from_redis_value(value)?;
        Ok(Some(Bytes::from(vec)))
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
