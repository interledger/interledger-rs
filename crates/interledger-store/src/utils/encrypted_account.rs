use super::account::{Account, AccountId};
use super::crypto::decrypt_token;
use interledger_btp::BtpAccount;
use interledger_ccp::{CcpRoutingAccount, RoutingRelation};
use interledger_http::HttpAccount;
use interledger_packet::Address;
use interledger_service::{Account as AccountTrait, Username};
use interledger_service_util::{
    MaxPacketAmountAccount, RateLimitAccount, RoundTripTimeAccount,
};
use interledger_settlement::{SettlementAccount, SettlementEngineDetails};
use log::error;

use ring::aead;
use serde::Serializer;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::{
    str::{self, FromStr},
};
use uuid::{parser::ParseError, Uuid};

use url::Url;

use secrecy::ExposeSecret;
use secrecy::SecretBytes;


#[derive(Debug, Clone)]
pub struct AccountWithEncryptedTokens {
    pub(crate) account: Account,
}

impl AccountWithEncryptedTokens {
    pub fn decrypt_tokens(mut self, decryption_key: &aead::OpeningKey) -> Account {
        if let Some(ref encrypted) = self.account.btp_outgoing_token {
            self.account.btp_outgoing_token =
                decrypt_token(decryption_key, &encrypted.expose_secret()).map(SecretBytes::from);
        }
        if let Some(ref encrypted) = self.account.http_outgoing_token {
            self.account.http_outgoing_token =
                decrypt_token(decryption_key, &encrypted.expose_secret()).map(SecretBytes::from);
        }
        if let Some(ref encrypted) = self.account.btp_incoming_token {
            self.account.btp_incoming_token =
                decrypt_token(decryption_key, &encrypted.expose_secret()).map(SecretBytes::from);
        }
        if let Some(ref encrypted) = self.account.http_incoming_token {
            self.account.http_incoming_token =
                decrypt_token(decryption_key, &encrypted.expose_secret()).map(SecretBytes::from);
        }

        self.account
    }
}

impl AccountTrait for AccountWithEncryptedTokens {
    type AccountId = AccountId;

    fn id(&self) -> Self::AccountId {
        self.account.id
    }

    fn username(&self) -> &Username {
        &self.account.username
    }

    fn ilp_address(&self) -> &Address {
        &self.account.ilp_address
    }

    fn asset_code(&self) -> &str {
        self.account.asset_code.as_str()
    }

    fn asset_scale(&self) -> u8 {
        self.account.asset_scale
    }
}

impl HttpAccount for AccountWithEncryptedTokens {
    fn get_http_url(&self) -> Option<&Url> {
        self.account.http_endpoint.as_ref()
    }

    fn get_http_auth_token(&self) -> Option<&str> {
        self.account.http_outgoing_token
            .as_ref()
            .map(|s| str::from_utf8(s.expose_secret().as_ref()).unwrap_or_default())
    }
}

impl BtpAccount for AccountWithEncryptedTokens {
    fn get_btp_uri(&self) -> Option<&Url> {
        self.account.btp_uri.as_ref()
    }

    fn get_btp_token(&self) -> Option<&[u8]> {
        if let Some(ref token) = self.account.btp_outgoing_token {
            Some(&token.expose_secret())
        } else {
            None
        }
    }
}

impl MaxPacketAmountAccount for AccountWithEncryptedTokens {
    fn max_packet_amount(&self) -> u64 {
        self.account.max_packet_amount
    }
}

impl CcpRoutingAccount for AccountWithEncryptedTokens {
    fn routing_relation(&self) -> RoutingRelation {
        self.account.routing_relation
    }
}

impl RoundTripTimeAccount for AccountWithEncryptedTokens {
    fn round_trip_time(&self) -> u32 {
        self.account.round_trip_time
    }
}

impl RateLimitAccount for AccountWithEncryptedTokens {
    fn amount_per_minute_limit(&self) -> Option<u64> {
        self.account.amount_per_minute_limit
    }

    fn packets_per_minute_limit(&self) -> Option<u32> {
        self.account.packets_per_minute_limit
    }
}

impl SettlementAccount for AccountWithEncryptedTokens {
    fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
        match &self.account.settlement_engine_url {
            Some(url) => Some(SettlementEngineDetails { url: url.clone() }),
            _ => None,
        }
    }
}
