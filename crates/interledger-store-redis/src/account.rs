use super::crypto::{decrypt_token, encrypt_token};
use bytes::Bytes;
use interledger_api::AccountDetails;
use interledger_ccp::RoutingRelation;
use interledger_packet::Address;
use interledger_service::{Account, AccountId, AccountWithEncryptedTokens, Username};
use interledger_service_util::DEFAULT_ROUND_TRIP_TIME;
use interledger_settlement::SettlementEngineDetails;
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

use secrecy::ExposeSecret;
use secrecy::SecretBytes;

pub fn account_try_from(
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
    let settlement_engine_url = if let Some(settlement_engine_url) = details.settlement_engine_url {
        Url::parse(&settlement_engine_url).ok()
    } else {
        None
    };

    Ok(Account::new(
        id,
        details.username,
        ilp_address,
        details.asset_code.to_uppercase(),
        details.asset_scale,
        details.max_packet_amount,
        details.min_balance,
        ilp_over_http_url,
        details
            .ilp_over_http_incoming_token
            .map(|token| SecretBytes::new(token.expose_secret().to_string())),
        details
            .ilp_over_http_outgoing_token
            .map(|token| SecretBytes::new(token.expose_secret().to_string())),
        ilp_over_btp_url,
        details
            .ilp_over_btp_incoming_token
            .map(|token| SecretBytes::new(token.expose_secret().to_string())),
        details
            .ilp_over_btp_outgoing_token
            .map(|token| SecretBytes::new(token.expose_secret().to_string())),
        details.settle_threshold,
        details.settle_to,
        routing_relation,
        details.round_trip_time.unwrap_or(DEFAULT_ROUND_TRIP_TIME),
        details.packets_per_minute_limit,
        details.amount_per_minute_limit,
        settlement_engine_url,
    ))
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
        let account = account_try_from(
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
