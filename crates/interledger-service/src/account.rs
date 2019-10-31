use crate::{decrypt_token, encrypt_token, RoutingRelation, Username};

use std::{
    fmt::Display,
    str::{self, FromStr},
};

use interledger_packet::Address;

use log::error;
use ring::aead;
use secrecy::{ExposeSecret, SecretBytes};
use serde::{Deserialize, Serialize, Serializer};
use url::Url;
use uuid::{parser::ParseError, Uuid};

fn address_to_string<S>(address: &Address, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(str::from_utf8(address.as_ref()).unwrap_or(""))
}

fn optional_secret_bytes_to_utf8<S>(
    _bytes: &Option<SecretBytes>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str("SECRET")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub(crate) id: AccountId,
    pub(crate) username: Username,
    #[serde(serialize_with = "address_to_string")]
    pub(crate) ilp_address: Address,
    // TODO add additional routes
    pub(crate) asset_code: String,
    pub(crate) asset_scale: u8,
    pub(crate) max_packet_amount: u64,
    pub(crate) min_balance: Option<i64>,
    pub(crate) ilp_over_http_url: Option<Url>,
    #[serde(serialize_with = "optional_secret_bytes_to_utf8")]
    pub(crate) ilp_over_http_incoming_token: Option<SecretBytes>,
    #[serde(serialize_with = "optional_secret_bytes_to_utf8")]
    pub(crate) ilp_over_http_outgoing_token: Option<SecretBytes>,
    pub(crate) ilp_over_btp_url: Option<Url>,
    #[serde(serialize_with = "optional_secret_bytes_to_utf8")]
    pub(crate) ilp_over_btp_incoming_token: Option<SecretBytes>,
    #[serde(serialize_with = "optional_secret_bytes_to_utf8")]
    pub(crate) ilp_over_btp_outgoing_token: Option<SecretBytes>,
    pub(crate) settle_threshold: Option<i64>,
    pub(crate) settle_to: Option<i64>,
    pub(crate) routing_relation: RoutingRelation,
    pub(crate) round_trip_time: u32,
    pub(crate) packets_per_minute_limit: Option<u32>,
    pub(crate) amount_per_minute_limit: Option<u64>,
    pub(crate) settlement_engine_url: Option<Url>,
}

impl Account {
    pub fn new(
        id: AccountId,
        username: Username,
        ilp_address: Address,
        asset_code: String,
        asset_scale: u8,
        max_packet_amount: u64,
        min_balance: Option<i64>,
        ilp_over_http_url: Option<Url>,
        ilp_over_http_incoming_token: Option<SecretBytes>,
        ilp_over_http_outgoing_token: Option<SecretBytes>,
        ilp_over_btp_url: Option<Url>,
        ilp_over_btp_incoming_token: Option<SecretBytes>,
        ilp_over_btp_outgoing_token: Option<SecretBytes>,
        settle_threshold: Option<i64>,
        settle_to: Option<i64>,
        routing_relation: RoutingRelation,
        round_trip_time: u32,
        packets_per_minute_limit: Option<u32>,
        amount_per_minute_limit: Option<u64>,
        settlement_engine_url: Option<Url>,
    ) -> Self {
        Account {
            id,
            username,
            ilp_address,
            asset_code,
            asset_scale,
            max_packet_amount,
            min_balance,
            ilp_over_http_url,
            ilp_over_http_incoming_token,
            ilp_over_http_outgoing_token,
            ilp_over_btp_url,
            ilp_over_btp_incoming_token,
            ilp_over_btp_outgoing_token,
            settle_threshold,
            settle_to,
            routing_relation,
            round_trip_time,
            packets_per_minute_limit,
            amount_per_minute_limit,
            settlement_engine_url,
        }
    }

    pub fn encrypt_tokens(
        mut self,
        encryption_key: &aead::LessSafeKey,
    ) -> AccountWithEncryptedTokens {
        if let Some(ref token) = self.ilp_over_btp_outgoing_token {
            self.ilp_over_btp_outgoing_token = Some(SecretBytes::from(encrypt_token(
                encryption_key,
                &token.expose_secret(),
            )));
        }
        if let Some(ref token) = self.ilp_over_http_outgoing_token {
            self.ilp_over_http_outgoing_token = Some(SecretBytes::from(encrypt_token(
                encryption_key,
                &token.expose_secret(),
            )));
        }
        if let Some(ref token) = self.ilp_over_btp_incoming_token {
            self.ilp_over_btp_incoming_token = Some(SecretBytes::from(encrypt_token(
                encryption_key,
                &token.expose_secret(),
            )));
        }
        if let Some(ref token) = self.ilp_over_http_incoming_token {
            self.ilp_over_http_incoming_token = Some(SecretBytes::from(encrypt_token(
                encryption_key,
                &token.expose_secret(),
            )));
        }
        AccountWithEncryptedTokens { account: self }
    }

    // XXX: should this exist?
    pub fn ilp_over_btp_incoming_token(&self) -> Option<SecretBytes> {
        self.ilp_over_btp_incoming_token.clone()
    }

    // XXX: should this exist?
    pub fn ilp_over_http_incoming_token(&self) -> Option<SecretBytes> {
        self.ilp_over_http_incoming_token.clone()
    }

    // Account

    pub fn id(&self) -> AccountId {
        self.id
    }

    pub fn username(&self) -> &Username {
        &self.username
    }

    pub fn ilp_address(&self) -> &Address {
        &self.ilp_address
    }

    pub fn asset_code(&self) -> &str {
        self.asset_code.as_str()
    }

    pub fn asset_scale(&self) -> u8 {
        self.asset_scale
    }

    // HttpAccount

    pub fn get_http_url(&self) -> Option<&Url> {
        self.ilp_over_http_url.as_ref()
    }

    pub fn get_http_auth_token(&self) -> Option<&str> {
        self.ilp_over_http_outgoing_token
            .as_ref()
            .map(|s| str::from_utf8(s.expose_secret().as_ref()).unwrap_or_default())
    }

    // BtpAccount

    pub fn get_ilp_over_btp_url(&self) -> Option<&Url> {
        self.ilp_over_btp_url.as_ref()
    }

    pub fn get_ilp_over_btp_outgoing_token(&self) -> Option<&[u8]> {
        if let Some(ref token) = self.ilp_over_btp_outgoing_token {
            Some(&token.expose_secret())
        } else {
            None
        }
    }

    // MaxPacketAmountAccount

    pub fn max_packet_amount(&self) -> u64 {
        self.max_packet_amount
    }

    // CcpRoutingAccount

    pub fn routing_relation(&self) -> RoutingRelation {
        self.routing_relation
    }

    // RoundTripTimeAccount

    pub fn round_trip_time(&self) -> u32 {
        self.round_trip_time
    }

    // RateLimitAccount

    pub fn amount_per_minute_limit(&self) -> Option<u64> {
        self.amount_per_minute_limit
    }

    pub fn packets_per_minute_limit(&self) -> Option<u32> {
        self.packets_per_minute_limit
    }

    // SettlementAccount

    pub fn settlement_engine_details(&self) -> Option<Url> {
        self.settlement_engine_url.clone()
    }

    // CcpRoutingAccount

    /// Indicates whether we should send CCP Route Updates to this account
    pub fn should_send_routes(&self) -> bool {
        self.routing_relation() == RoutingRelation::Child
            || self.routing_relation() == RoutingRelation::Peer
    }

    /// Indicates whether we should accept CCP Route Update Requests from this account
    pub fn should_receive_routes(&self) -> bool {
        self.routing_relation() == RoutingRelation::Parent
            || self.routing_relation() == RoutingRelation::Peer
    }
}

#[derive(Debug, Clone)]
pub struct AccountWithEncryptedTokens {
    pub(super) account: Account,
}

impl AccountWithEncryptedTokens {
    // XXX: what's the right thing to do here?
    pub fn account(self) -> Account {
        self.account
    }

    pub fn decrypt_tokens(mut self, decryption_key: &aead::LessSafeKey) -> Account {
        if let Some(ref encrypted) = self.account.ilp_over_btp_outgoing_token {
            self.account.ilp_over_btp_outgoing_token =
                decrypt_token(decryption_key, &encrypted.expose_secret())
                    .map_err(|_| {
                        error!(
                            "Unable to decrypt ilp_over_btp_outgoing_token for account {}",
                            self.account.id
                        )
                    })
                    .ok();
        }
        if let Some(ref encrypted) = self.account.ilp_over_http_outgoing_token {
            self.account.ilp_over_http_outgoing_token =
                decrypt_token(decryption_key, &encrypted.expose_secret())
                    .map_err(|_| {
                        error!(
                            "Unable to decrypt ilp_over_http_outgoing_token for account {}",
                            self.account.id
                        )
                    })
                    .ok();
        }
        if let Some(ref encrypted) = self.account.ilp_over_btp_incoming_token {
            self.account.ilp_over_btp_incoming_token =
                decrypt_token(decryption_key, &encrypted.expose_secret())
                    .map_err(|_| {
                        error!(
                            "Unable to decrypt ilp_over_btp_incoming_token for account {}",
                            self.account.id
                        )
                    })
                    .ok();
        }
        if let Some(ref encrypted) = self.account.ilp_over_http_incoming_token {
            self.account.ilp_over_http_incoming_token =
                decrypt_token(decryption_key, &encrypted.expose_secret())
                    .map_err(|_| {
                        error!(
                            "Unable to decrypt ilp_over_http_incoming_token for account {}",
                            self.account.id
                        )
                    })
                    .ok();
        }

        self.account
    }
}

#[derive(Eq, PartialEq, Hash, Debug, Default, Serialize, Deserialize, Copy, Clone)]
pub struct AccountId(pub Uuid);

impl AccountId {
    pub fn new() -> Self {
        let uid = Uuid::new_v4();
        AccountId(uid)
    }
}

impl FromStr for AccountId {
    type Err = ParseError;

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        let uid = Uuid::from_str(&src)?;
        Ok(AccountId(uid))
    }
}

impl Display for AccountId {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> Result<(), ::std::fmt::Error> {
        f.write_str(&self.0.to_hyphenated().to_string())
    }
}

#[cfg(feature = "redis")]
use redis::{
    from_redis_value, ErrorKind, FromRedisValue, RedisError, RedisWrite, ToRedisArgs, Value,
};

#[cfg(feature = "redis")]
use std::collections::HashMap;

#[cfg(feature = "redis")]
use bytes::Bytes;

#[cfg(feature = "redis")]
const ACCOUNT_DETAILS_FIELDS: usize = 21;

#[cfg(feature = "redis")]
const DEFAULT_ROUND_TRIP_TIME: u32 = 500;

#[cfg(feature = "redis")]
impl ToRedisArgs for AccountId {
    fn write_redis_args<W: RedisWrite + ?Sized>(&self, out: &mut W) {
        out.write_arg(self.0.to_hyphenated().to_string().as_bytes().as_ref());
    }
}

#[cfg(feature = "redis")]
impl FromRedisValue for AccountId {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        let account_id = String::from_redis_value(v)?;
        let uid = Uuid::from_str(&account_id)
            .map_err(|_| RedisError::from((ErrorKind::TypeError, "Invalid account id string")))?;
        Ok(AccountId(uid))
    }
}

#[cfg(feature = "redis")]
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
        if let Some(ilp_over_http_url) = account.ilp_over_http_url.as_ref() {
            "ilp_over_http_url".write_redis_args(&mut rv);
            ilp_over_http_url.as_str().write_redis_args(&mut rv);
        }
        if let Some(ilp_over_http_incoming_token) = account.ilp_over_http_incoming_token.as_ref() {
            "ilp_over_http_incoming_token".write_redis_args(&mut rv);
            ilp_over_http_incoming_token
                .expose_secret()
                .as_ref()
                .write_redis_args(&mut rv);
        }
        if let Some(ilp_over_http_outgoing_token) = account.ilp_over_http_outgoing_token.as_ref() {
            "ilp_over_http_outgoing_token".write_redis_args(&mut rv);
            ilp_over_http_outgoing_token
                .expose_secret()
                .as_ref()
                .write_redis_args(&mut rv);
        }
        if let Some(ilp_over_btp_url) = account.ilp_over_btp_url.as_ref() {
            "ilp_over_btp_url".write_redis_args(&mut rv);
            ilp_over_btp_url.as_str().write_redis_args(&mut rv);
        }
        if let Some(ilp_over_btp_incoming_token) = account.ilp_over_btp_incoming_token.as_ref() {
            "ilp_over_btp_incoming_token".write_redis_args(&mut rv);
            ilp_over_btp_incoming_token
                .expose_secret()
                .as_ref()
                .write_redis_args(&mut rv);
        }
        if let Some(ilp_over_btp_outgoing_token) = account.ilp_over_btp_outgoing_token.as_ref() {
            "ilp_over_btp_outgoing_token".write_redis_args(&mut rv);
            ilp_over_btp_outgoing_token
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

#[cfg(feature = "redis")]
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
            RoutingRelation::NonRoutingAccount
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
                ilp_over_http_url: get_url_option("ilp_over_http_url", &hash)?,
                ilp_over_http_incoming_token: get_bytes_option(
                    "ilp_over_http_incoming_token",
                    &hash,
                )?
                .map(SecretBytes::from),
                ilp_over_http_outgoing_token: get_bytes_option(
                    "ilp_over_http_outgoing_token",
                    &hash,
                )?
                .map(SecretBytes::from),
                ilp_over_btp_url: get_url_option("ilp_over_btp_url", &hash)?,
                ilp_over_btp_incoming_token: get_bytes_option(
                    "ilp_over_btp_incoming_token",
                    &hash,
                )?
                .map(SecretBytes::from),
                ilp_over_btp_outgoing_token: get_bytes_option(
                    "ilp_over_btp_outgoing_token",
                    &hash,
                )?
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

#[cfg(feature = "redis")]
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

#[cfg(feature = "redis")]
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

#[cfg(feature = "redis")]
fn get_bytes_option(key: &str, map: &HashMap<String, Value>) -> Result<Option<Bytes>, RedisError> {
    if let Some(ref value) = map.get(key) {
        let vec: Vec<u8> = from_redis_value(value)?;
        Ok(Some(Bytes::from(vec)))
    } else {
        Ok(None)
    }
}

#[cfg(feature = "redis")]
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
