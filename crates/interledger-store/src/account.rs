use super::crypto::{decrypt_token, encrypt_token};
use interledger_api::AccountDetails;
use interledger_btp::BtpAccount;
use interledger_ccp::{CcpRoutingAccount, RoutingRelation};
use interledger_errors::CreateAccountError;
use interledger_http::HttpAccount;
use interledger_packet::Address;
use interledger_service::{Account as AccountTrait, Username};
use interledger_service_util::{
    MaxPacketAmountAccount, RateLimitAccount, RoundTripTimeAccount, DEFAULT_ROUND_TRIP_TIME,
};
use interledger_settlement::core::types::{SettlementAccount, SettlementEngineDetails};
use ring::aead;
use secrecy::{ExposeSecret, SecretBytesMut, SecretString};
use serde::Serializer;
use serde::{Deserialize, Serialize};
use std::str::{self, FromStr};
use tracing::error;
use url::Url;
use uuid::Uuid;

/// The account which contains all the data required for a full implementation of Interledger
// TODO: Maybe we should feature gate these fields? e.g. ilp_over_btp variables should only be there
// if btp feature is enabled
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    /// Unique id corresponding to the account's primary key in the database
    pub(crate) id: Uuid,
    /// The account's username
    pub(crate) username: Username,
    #[serde(serialize_with = "address_to_string")]
    /// The account's Interledger Protocol address
    pub(crate) ilp_address: Address,
    // TODO add additional routes
    /// The account's currency
    pub(crate) asset_code: String,
    /// The account's asset scale
    pub(crate) asset_scale: u8,
    /// The max amount per packet which can be routed for this account
    pub(crate) max_packet_amount: u64,
    /// The minimum balance this account can have (consider this as a credit/trust limit)
    pub(crate) min_balance: Option<i64>,
    /// The account's ILP over HTTP URL (this is where packets are sent over HTTP from your node)
    pub(crate) ilp_over_http_url: Option<Url>,
    #[serde(serialize_with = "optional_secret_bytes_to_utf8")]
    /// The account's API and incoming ILP over HTTP token.
    /// This must match the ILP over HTTP outgoing token on the peer's node if receiving
    /// packets from that peer
    // TODO: The incoming token is used for both ILP over HTTP, and for authorizing actions from the HTTP API.
    // Should we add 1 more token, for more granular permissioning?
    pub(crate) ilp_over_http_incoming_token: Option<SecretBytesMut>,
    #[serde(serialize_with = "optional_secret_bytes_to_utf8")]
    /// The account's outgoing ILP over HTTP token
    /// This must match the ILP over HTTP incoming token on the peer's node if sending
    /// packets to that peer
    pub(crate) ilp_over_http_outgoing_token: Option<SecretBytesMut>,
    /// The account's ILP over BTP URL (this is where packets are sent over WebSockets from your node)
    pub(crate) ilp_over_btp_url: Option<Url>,
    #[serde(serialize_with = "optional_secret_bytes_to_utf8")]
    /// The account's incoming ILP over BTP token.
    /// This must match the ILP over BTP outgoing token on the peer's node if exchanging
    /// packets with that peer
    pub(crate) ilp_over_btp_incoming_token: Option<SecretBytesMut>,
    #[serde(serialize_with = "optional_secret_bytes_to_utf8")]
    /// The account's outgoing ILP over BTP token.
    /// This must match the ILP over BTP incoming token on the peer's node if exchanging
    /// packets with that peer
    pub(crate) ilp_over_btp_outgoing_token: Option<SecretBytesMut>,
    /// The threshold after which the balance service will trigger a settlement
    pub(crate) settle_threshold: Option<i64>,
    /// The amount which the balance service will attempt to settle down to
    pub(crate) settle_to: Option<i64>,
    /// The routing relation of the account
    pub(crate) routing_relation: RoutingRelation,
    /// The round trip time of the account (should be set depending on how
    /// well the network connectivity of the account and the node is)
    pub(crate) round_trip_time: u32,
    /// The limit of packets the account can send per minute
    pub(crate) packets_per_minute_limit: Option<u32>,
    /// The maximum amount the account can send per minute
    pub(crate) amount_per_minute_limit: Option<u64>,
    /// The account's settlement engine URL. If a global engine url is configured
    /// for the account's asset code,  that will be used instead (even if the account is
    /// configured with a specific one)
    pub(crate) settlement_engine_url: Option<Url>,
}

fn address_to_string<S>(address: &Address, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(str::from_utf8(address.as_ref()).unwrap_or(""))
}

fn optional_secret_bytes_to_utf8<S>(
    _bytes: &Option<SecretBytesMut>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str("SECRET")
}

impl Account {
    /// Creates an account from the provided id and details. If there is no ILP Address
    /// in the provided details, then the account's ILP Address is generated by appending
    /// the `details.username` to the provided `node_ilp_address`.
    /// The default RoutingRelation is `NonRoutingAccount`
    pub fn try_from(
        id: Uuid,
        details: AccountDetails,
        node_ilp_address: Address,
    ) -> Result<Account, CreateAccountError> {
        let ilp_address = match details.ilp_address {
            Some(a) => a,
            None => node_ilp_address
                .with_suffix(details.username.as_bytes())
                .map_err(CreateAccountError::InvalidSuffix)?,
        };

        let ilp_over_http_url = if let Some(ref url) = details.ilp_over_http_url {
            Some(Url::parse(url).map_err(CreateAccountError::InvalidHttpUrl)?)
        } else {
            None
        };

        let ilp_over_btp_url = if let Some(ref url) = details.ilp_over_btp_url {
            Some(Url::parse(url).map_err(CreateAccountError::InvalidBtpUrl)?)
        } else {
            None
        };

        let routing_relation = if let Some(ref relation) = details.routing_relation {
            RoutingRelation::from_str(relation)
                .map_err(|_| CreateAccountError::InvalidRoutingRelation(relation.to_string()))?
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
                .map(|token| SecretBytesMut::new(token.expose_secret().as_str())),
            ilp_over_http_outgoing_token: details
                .ilp_over_http_outgoing_token
                .map(|token| SecretBytesMut::new(token.expose_secret().as_str())),
            ilp_over_btp_url,
            ilp_over_btp_incoming_token: details
                .ilp_over_btp_incoming_token
                .map(|token| SecretBytesMut::new(token.expose_secret().as_str())),
            ilp_over_btp_outgoing_token: details
                .ilp_over_btp_outgoing_token
                .map(|token| SecretBytesMut::new(token.expose_secret().as_str())),
            settle_to: details.settle_to,
            settle_threshold: details.settle_threshold,
            routing_relation,
            round_trip_time: details.round_trip_time.unwrap_or(DEFAULT_ROUND_TRIP_TIME),
            packets_per_minute_limit: details.packets_per_minute_limit,
            amount_per_minute_limit: details.amount_per_minute_limit,
            settlement_engine_url,
        })
    }

    /// Encrypts the account's incoming/outgoing BTP and HTTP keys with the provided encryption key
    pub fn encrypt_tokens(
        mut self,
        encryption_key: &aead::LessSafeKey,
    ) -> AccountWithEncryptedTokens {
        if let Some(ref token) = self.ilp_over_btp_outgoing_token {
            self.ilp_over_btp_outgoing_token = Some(SecretBytesMut::from(encrypt_token(
                encryption_key,
                &token.expose_secret(),
            )));
        }
        if let Some(ref token) = self.ilp_over_http_outgoing_token {
            self.ilp_over_http_outgoing_token = Some(SecretBytesMut::from(encrypt_token(
                encryption_key,
                &token.expose_secret(),
            )));
        }
        if let Some(ref token) = self.ilp_over_btp_incoming_token {
            self.ilp_over_btp_incoming_token = Some(SecretBytesMut::from(encrypt_token(
                encryption_key,
                &token.expose_secret(),
            )));
        }
        if let Some(ref token) = self.ilp_over_http_incoming_token {
            self.ilp_over_http_incoming_token = Some(SecretBytesMut::from(encrypt_token(
                encryption_key,
                &token.expose_secret(),
            )));
        }
        AccountWithEncryptedTokens { account: self }
    }
}

/// A wrapper over the [`Account`](./struct.Account.html) which contains their encrypt tokens.
#[derive(Debug, Clone)]
pub struct AccountWithEncryptedTokens {
    pub(super) account: Account,
}

impl AccountWithEncryptedTokens {
    /// Decrypts the account's incoming/outgoing BTP and HTTP keys with the provided decryption key
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

// The following trait implementations are simple accessors to the Account's fields

impl AccountTrait for Account {
    fn id(&self) -> Uuid {
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

    fn get_http_auth_token(&self) -> Option<SecretString> {
        self.ilp_over_http_outgoing_token.as_ref().map(|s| {
            SecretString::new(
                str::from_utf8(s.expose_secret().as_ref())
                    .unwrap_or_default()
                    .to_string(),
            )
        })
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
mod test {
    use super::*;
    use once_cell::sync::Lazy;
    use secrecy::SecretString;

    static ACCOUNT_DETAILS: Lazy<AccountDetails> = Lazy::new(|| AccountDetails {
        ilp_address: Some(Address::from_str("example.alice").unwrap()),
        username: Username::from_str("alice").unwrap(),
        asset_scale: 6,
        asset_code: "XYZ".to_string(),
        max_packet_amount: 1000,
        min_balance: Some(-1000),
        // we are Bob and we're using this account to peer with Alice
        ilp_over_http_url: Some("http://example.com/accounts/bob/ilp".to_string()),
        ilp_over_http_incoming_token: Some(SecretString::new("incoming_auth_token".to_string())),
        ilp_over_http_outgoing_token: Some(SecretString::new("outgoing_auth_token".to_string())),
        ilp_over_btp_url: Some("btp+ws://example.com/accounts/bob/ilp/btp".to_string()),
        ilp_over_btp_incoming_token: Some(SecretString::new("incoming_btp_token".to_string())),
        ilp_over_btp_outgoing_token: Some(SecretString::new("outgoing_btp_token".to_string())),
        settle_threshold: Some(0),
        settle_to: Some(-1000),
        routing_relation: Some("Peer".to_string()),
        round_trip_time: Some(600),
        amount_per_minute_limit: None,
        packets_per_minute_limit: None,
        settlement_engine_url: None,
    });

    #[test]
    fn from_account_details() {
        let id = Uuid::new_v4();
        let account = Account::try_from(
            id,
            ACCOUNT_DETAILS.clone(),
            Address::from_str("example.account").unwrap(),
        )
        .unwrap();
        assert_eq!(account.id(), id);
        // the HTTP token does not contain the username
        assert_eq!(
            account.get_http_auth_token().unwrap().expose_secret(),
            "outgoing_auth_token",
        );
        assert_eq!(
            account.get_ilp_over_btp_outgoing_token().unwrap(),
            b"outgoing_btp_token",
        );
        assert_eq!(
            account.get_ilp_over_btp_url().unwrap().to_string(),
            "btp+ws://example.com/accounts/bob/ilp/btp",
        );
        assert_eq!(
            account.get_http_url().unwrap().to_string(),
            "http://example.com/accounts/bob/ilp",
        );
        assert_eq!(account.routing_relation(), RoutingRelation::Peer);
    }
}
