use super::crypto::{decrypt_token, encrypt_token};
use bytes::Bytes;
use interledger_api::AccountDetails;
use interledger_btp::BtpAccount;
use interledger_ccp::{CcpRoutingAccount, RoutingRelation};
use interledger_http::HttpAccount;
use interledger_packet::Address;
use interledger_service::{Account as AccountTrait, Username};
use interledger_service_util::{
    MaxPacketAmountAccount, RateLimitAccount, RoundTripTimeAccount, DEFAULT_ROUND_TRIP_TIME,
};
use interledger_settlement::{SettlementAccount, SettlementEngineDetails};
use log::error;
use redis::{
    from_redis_value, ErrorKind, FromRedisValue, RedisError, RedisWrite, ToRedisArgs, Value,
};

use ring::aead;
use serde::Serializer;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::{
    collections::HashMap,
    str::{self, FromStr},
};
use uuid::{parser::ParseError, Uuid};

use url::Url;
const ACCOUNT_DETAILS_FIELDS: usize = 21;

use secrecy::ExposeSecret;
use secrecy::SecretBytes;

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

impl Account {
    pub fn try_from(
        id: AccountId,
        details: AccountDetails,
        node_ilp_address: Address,
    ) -> Result<Account, ()> {
        let ilp_address = match details.ilp_address {
            Some(a) => a,
            None => node_ilp_address
                .with_suffix(details.username.as_bytes())
                .map_err(|_| {
                    error!(
                        "Could not append username {} to address {}",
                        details.username, node_ilp_address
                    )
                })?,
        };

        let ilp_over_http_url = if let Some(ref url) = details.ilp_over_http_url {
            Some(Url::parse(url).map_err(|err| error!("Invalid URL: {:?}", err))?)
        } else {
            None
        };

        let ilp_over_btp_url = if let Some(ref url) = details.ilp_over_btp_url {
            Some(Url::parse(url).map_err(|err| error!("Invalid URL: {:?}", err))?)
        } else {
            None
        };

        let routing_relation = if let Some(ref relation) = details.routing_relation {
            RoutingRelation::from_str(relation)?
        } else {
            RoutingRelation::NonRoutingAccount
        };
        let settlement_engine_url =
            if let Some(settlement_engine_url) = details.settlement_engine_url {
                Url::parse(&settlement_engine_url).ok()
            } else {
                None
            };

        Ok(Account {
            id,
            username: details.username,
            ilp_address,
            asset_code: details.asset_code.to_uppercase(),
            asset_scale: details.asset_scale,
            max_packet_amount: details.max_packet_amount,
            min_balance: details.min_balance,
            ilp_over_http_url,
            ilp_over_http_incoming_token: details
                .ilp_over_http_incoming_token
                .map(|token| SecretBytes::new(token.expose_secret().to_string())),
            ilp_over_http_outgoing_token: details
                .ilp_over_http_outgoing_token
                .map(|token| SecretBytes::new(token.expose_secret().to_string())),
            ilp_over_btp_url,
            ilp_over_btp_incoming_token: details
                .ilp_over_btp_incoming_token
                .map(|token| SecretBytes::new(token.expose_secret().to_string())),
            ilp_over_btp_outgoing_token: details
                .ilp_over_btp_outgoing_token
                .map(|token| SecretBytes::new(token.expose_secret().to_string())),
            settle_to: details.settle_to,
            settle_threshold: details.settle_threshold,
            routing_relation,
            round_trip_time: details.round_trip_time.unwrap_or(DEFAULT_ROUND_TRIP_TIME),
            packets_per_minute_limit: details.packets_per_minute_limit,
            amount_per_minute_limit: details.amount_per_minute_limit,
            settlement_engine_url,
        })
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
}

#[derive(Debug, Clone)]
pub struct AccountWithEncryptedTokens {
    pub(super) account: Account,
}

impl AccountWithEncryptedTokens {
    pub fn decrypt_tokens(mut self, decryption_key: &aead::LessSafeKey) -> Account {
        if let Some(ref encrypted) = self.account.ilp_over_btp_outgoing_token {
            self.account.ilp_over_btp_outgoing_token =
                decrypt_token(decryption_key, &encrypted.expose_secret()).map(SecretBytes::from);
        }
        if let Some(ref encrypted) = self.account.ilp_over_http_outgoing_token {
            self.account.ilp_over_http_outgoing_token =
                decrypt_token(decryption_key, &encrypted.expose_secret()).map(SecretBytes::from);
        }
        if let Some(ref encrypted) = self.account.ilp_over_btp_incoming_token {
            self.account.ilp_over_btp_incoming_token =
                decrypt_token(decryption_key, &encrypted.expose_secret()).map(SecretBytes::from);
        }
        if let Some(ref encrypted) = self.account.ilp_over_http_incoming_token {
            self.account.ilp_over_http_incoming_token =
                decrypt_token(decryption_key, &encrypted.expose_secret()).map(SecretBytes::from);
        }

        self.account
    }
}

// Uuid does not implement ToRedisArgs and FromRedisValue.
// Rust does not allow implementing foreign traits on foreign data types.
// As a result, we wrap Uuid in a local data type, and implement the necessary
// traits for that.
#[derive(Eq, PartialEq, Hash, Debug, Default, Serialize, Deserialize, Copy, Clone)]
pub struct AccountId(Uuid);

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

impl AccountTrait for Account {
    type AccountId = AccountId;

    fn id(&self) -> Self::AccountId {
        self.id
    }

    fn username(&self) -> &Username {
        &self.username
    }

    fn ilp_address(&self) -> &Address {
        &self.ilp_address
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
        self.ilp_over_http_url.as_ref()
    }

    fn get_http_auth_token(&self) -> Option<&str> {
        self.ilp_over_http_outgoing_token
            .as_ref()
            .map(|s| str::from_utf8(s.expose_secret().as_ref()).unwrap_or_default())
    }
}

impl BtpAccount for Account {
    fn get_ilp_over_btp_url(&self) -> Option<&Url> {
        self.ilp_over_btp_url.as_ref()
    }

    fn get_ilp_over_btp_outgoing_token(&self) -> Option<&[u8]> {
        if let Some(ref token) = self.ilp_over_btp_outgoing_token {
            Some(&token.expose_secret())
        } else {
            None
        }
    }
}

impl MaxPacketAmountAccount for Account {
    fn max_packet_amount(&self) -> u64 {
        self.max_packet_amount
    }
}

impl CcpRoutingAccount for Account {
    fn routing_relation(&self) -> RoutingRelation {
        self.routing_relation
    }
}

impl RoundTripTimeAccount for Account {
    fn round_trip_time(&self) -> u32 {
        self.round_trip_time
    }
}

impl RateLimitAccount for Account {
    fn amount_per_minute_limit(&self) -> Option<u64> {
        self.amount_per_minute_limit
    }

    fn packets_per_minute_limit(&self) -> Option<u32> {
        self.packets_per_minute_limit
    }
}

impl SettlementAccount for Account {
    fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
        match &self.settlement_engine_url {
            Some(url) => Some(SettlementEngineDetails { url: url.clone() }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod redis_account {
    use super::*;
    use lazy_static::lazy_static;
    use secrecy::SecretString;

    lazy_static! {
        static ref ACCOUNT_DETAILS: AccountDetails = AccountDetails {
            ilp_address: Some(Address::from_str("example.alice").unwrap()),
            username: Username::from_str("alice").unwrap(),
            asset_scale: 6,
            asset_code: "XYZ".to_string(),
            max_packet_amount: 1000,
            min_balance: Some(-1000),
            ilp_over_http_url: Some("http://example.com/ilp".to_string()),
            // we are Bob and we're using this account to peer with Alice
            ilp_over_http_incoming_token: Some(SecretString::new("incoming_auth_token".to_string())),
            ilp_over_http_outgoing_token: Some(SecretString::new("bob:outgoing_auth_token".to_string())),
            ilp_over_btp_url: Some("btp+ws://example.com/ilp/btp".to_string()),
            ilp_over_btp_incoming_token: Some(SecretString::new("alice:btp_token".to_string())),
            ilp_over_btp_outgoing_token: Some(SecretString::new("bob:btp_token".to_string())),
            settle_threshold: Some(0),
            settle_to: Some(-1000),
            routing_relation: Some("Peer".to_string()),
            round_trip_time: Some(600),
            amount_per_minute_limit: None,
            packets_per_minute_limit: None,
            settlement_engine_url: None,
        };
    }

    #[test]
    fn from_account_details() {
        let id = AccountId::new();
        let account = Account::try_from(
            id,
            ACCOUNT_DETAILS.clone(),
            Address::from_str("example.account").unwrap(),
        )
        .unwrap();
        assert_eq!(account.id(), id);
        assert_eq!(
            account.get_http_auth_token().unwrap(),
            format!("{}:outgoing_auth_token", "bob"),
        );
        assert_eq!(
            account.get_ilp_over_btp_outgoing_token().unwrap(),
            format!("{}:btp_token", "bob").as_bytes(),
        );
        assert_eq!(
            account.get_ilp_over_btp_url().unwrap().to_string(),
            "btp+ws://example.com/ilp/btp",
        );
        assert_eq!(account.routing_relation(), RoutingRelation::Peer);
    }
}
