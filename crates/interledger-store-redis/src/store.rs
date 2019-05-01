use super::account::*;
use super::crypto::generate_keys;
use bytes::Bytes;
use futures::{
    future::{err, ok, result, Either},
    Future, Stream,
};
use hashbrown::{HashMap, HashSet};
use interledger_api::{AccountDetails, NodeStore};
use interledger_btp::BtpStore;
use interledger_ccp::RouteManagerStore;
use interledger_http::HttpStore;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore};
use interledger_service_util::{BalanceStore, ExchangeRateStore, RateLimitError, RateLimitStore};
use parking_lot::RwLock;
use redis::{self, cmd, r#async::SharedConnection, Client, PipelineCommands, Value};
use ring::{aead, hmac};
use std::{
    iter::FromIterator,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_executor::spawn;
use tokio_timer::Interval;

const POLL_INTERVAL: u64 = 60000; // 1 minute

static ACCOUNT_FROM_INDEX: &str = "
local id = redis.call('HGET', KEYS[1], ARGV[1])
if not id then
    return nil
end
return redis.call('HGETALL', 'accounts:' .. id)";
static PROCESS_PREPARE: &str = "
local from_id = ARGV[1]
local from_account = 'accounts:' .. ARGV[1]
local from_amount = tonumber(ARGV[2])
local min_balance, balance, prepaid_amount = unpack(redis.call('HMGET', from_account, 'min_balance', 'balance', 'prepaid_amount'))
balance = tonumber(balance)
prepaid_amount = tonumber(balance)

-- Check that the prepare wouldn't go under the account's minimum balance
if min_balance then
    min_balance = tonumber(min_balance)
    if balance + prepaid_amount - from_amount < min_balance then
        error('Incoming prepare of ' .. from_amount .. ' would bring account ' .. from_id .. ' under its minimum balance. Current balance: ' .. balance .. ', min balance: ' .. min_balance)
    end
end

-- Deduct the from_amount from the prepaid_amount and/or the balance
if prepaid_amount >= from_amount then
    prepaid_amount = redis.call('HINCRBY', from_account, 'prepaid_amount', 0 - from_amount)
elseif prepaid_amount > 0 then
    local sub_from_balance = from_amount - prepaid_amount
    prepaid_amount = 0
    redis.call('HSET', from_account, 'prepaid_amount', 0)
    balance = redis.call('HINCRBY', from_account, 'balance', 0 - sub_from_balance)
else
    balance = redis.call('HINCRBY', from_account, 'balance', 0 - from_amount)
end

return balance + prepaid_amount";
static PROCESS_FULFILL: &str = "\
local to_account = 'accounts:' .. ARGV[1]
local to_amount = tonumber(ARGV[2])

local balance = redis.call('HINCRBY', to_account, 'balance', to_amount)
local prepaid_amount = redis.call('HGET', to_account, 'prepaid_amount')
return balance + prepaid_amount";
static PROCESS_REJECT: &str = "\
local from_account = 'accounts:' .. ARGV[1]
local from_amount = tonumber(ARGV[2])

local prepaid_amount = redis.call('HGET', from_account, 'prepaid_amount')
local balance = redis.call('HINCRBY', from_account, 'balance', from_amount)
return balance + prepaid_amount";

static ROUTES_KEY: &str = "routes:current";
static RATES_KEY: &str = "rates:current";
static STATIC_ROUTES_KEY: &str = "routes:static";
static NEXT_ACCOUNT_ID_KEY: &str = "next_account_id";

fn account_details_key(account_id: u64) -> String {
    format!("accounts:{}", account_id)
}

pub use redis::IntoConnectionInfo;

pub fn connect<R>(redis_uri: R, secret: [u8; 32]) -> impl Future<Item = RedisStore, Error = ()>
where
    R: IntoConnectionInfo,
{
    connect_with_poll_interval(redis_uri, secret, POLL_INTERVAL)
}

#[doc(hidden)]
pub fn connect_with_poll_interval<R>(
    redis_uri: R,
    secret: [u8; 32],
    poll_interval: u64,
) -> impl Future<Item = RedisStore, Error = ()>
where
    R: IntoConnectionInfo,
{
    result(Client::open(redis_uri))
        .map_err(|err| error!("Error creating Redis client: {:?}", err))
        .and_then(|client| {
            debug!("Connected to redis: {:?}", client);
            client
                .get_shared_async_connection()
                .map_err(|err| error!("Error connecting to Redis: {:?}", err))
        })
        .and_then(move |connection| {
            let (hmac_key, encryption_key, decryption_key) = generate_keys(&secret[..]);
            let store = RedisStore {
                connection: Arc::new(connection),
                exchange_rates: Arc::new(RwLock::new(HashMap::new())),
                routes: Arc::new(RwLock::new(HashMap::new())),
                hmac_key: Arc::new(hmac_key),
                encryption_key: Arc::new(encryption_key),
                decryption_key: Arc::new(decryption_key),
            };

            // Start polling for rate updates
            // Note: if this behavior changes, make sure to update the Drop implementation
            let connection_clone = Arc::downgrade(&store.connection);
            let exchange_rates = store.exchange_rates.clone();
            let poll_rates = Interval::new(Instant::now(), Duration::from_millis(poll_interval))
                .map_err(|err| error!("Interval error: {:?}", err))
                .for_each(move |_| {
                    if let Some(connection) = connection_clone.upgrade() {
                        Either::A(update_rates(
                            connection.as_ref().clone(),
                            exchange_rates.clone(),
                        ))
                    } else {
                        debug!("Not polling rates anymore because connection was closed");
                        // TODO make sure the interval stops
                        Either::B(err(()))
                    }
                });
            spawn(poll_rates);

            // Poll for routing table updates
            // Note: if this behavior changes, make sure to update the Drop implementation
            let connection_clone = Arc::downgrade(&store.connection);
            let routing_table = store.routes.clone();
            let poll_routes = Interval::new(Instant::now(), Duration::from_millis(poll_interval))
                .map_err(|err| error!("Interval error: {:?}", err))
                .for_each(move |_| {
                    if let Some(connection) = connection_clone.upgrade() {
                        Either::A(update_routes(
                            connection.as_ref().clone(),
                            routing_table.clone(),
                        ))
                    } else {
                        debug!("Not polling routes anymore because connection was closed");
                        // TODO make sure the interval stops
                        Either::B(err(()))
                    }
                });
            spawn(poll_routes);

            Ok(store)
        })
}

/// A Store that uses Redis as its underlying database.
///
/// This store leverages atomic Redis transactions to do operations such as balance updates.
///
/// Currently the RedisStore polls the database for the routing table and rate updates, but
/// future versions of it will use PubSub to subscribe to updates.
#[derive(Clone)]
pub struct RedisStore {
    connection: Arc<SharedConnection>,
    exchange_rates: Arc<RwLock<HashMap<String, f64>>>,
    routes: Arc<RwLock<HashMap<Bytes, u64>>>,
    hmac_key: Arc<hmac::SigningKey>,
    encryption_key: Arc<aead::SealingKey>,
    decryption_key: Arc<aead::OpeningKey>,
}

impl RedisStore {
    fn get_next_account_id(&self) -> impl Future<Item = u64, Error = ()> {
        cmd("INCR")
            .arg(NEXT_ACCOUNT_ID_KEY)
            .query_async(self.connection.as_ref().clone())
            .map_err(|err| error!("Error incrementing account ID: {:?}", err))
            .and_then(|(_conn, next_account_id): (_, u64)| Ok(next_account_id - 1))
    }

    fn create_new_account(
        &self,
        account: AccountDetails,
    ) -> Box<Future<Item = Account, Error = ()> + Send> {
        debug!("Inserting account: {:?}", account);
        let connection = self.connection.clone();
        let routing_table = self.routes.clone();
        let encryption_key = self.encryption_key.clone();

        // Instead of storing the incoming secrets, we store the HMAC digest of them
        // (This is better than encrypting because the output is deterministic so we can look
        // up the account by the HMAC of the auth details submitted by the account holder over the wire)
        let btp_incoming_token_hmac = account
            .btp_incoming_token
            .clone()
            .map(|token| hmac::sign(&self.hmac_key, token.as_bytes()));
        let btp_incoming_token_hmac_clone = btp_incoming_token_hmac;
        let http_incoming_token_hmac = account
            .http_incoming_token
            .clone()
            .map(|token| hmac::sign(&self.hmac_key, token.as_bytes()));
        let http_incoming_token_hmac_clone = http_incoming_token_hmac;

        Box::new(
            self.get_next_account_id()
                .and_then(|id| {
                    debug!("Next account id is: {}", id);
                    Account::try_from(id, account)
                })
                .and_then(move |account| {
                    // Check that there isn't already an account with values that must be unique
                    let mut keys: Vec<String> = vec!["ID".to_string()];

                    let mut pipe = redis::pipe();
                    pipe.exists(account_details_key(account.id));

                    if let Some(auth) = btp_incoming_token_hmac {
                        keys.push("BTP auth".to_string());
                        pipe.hexists("btp_auth", auth.as_ref());
                    }
                    if let Some(auth) = http_incoming_token_hmac {
                        keys.push("HTTP auth".to_string());
                        pipe.hexists("http_auth", auth.as_ref());
                    }
                    if let Some(ref xrp_address) = account.xrp_address {
                        keys.push("XRP address".to_string());
                        pipe.hexists("xrp_addresses", xrp_address);
                    }

                    pipe.query_async(connection.as_ref().clone())
                        .map_err(|err| {
                            error!(
                                "Error checking whether account details already exist: {:?}",
                                err
                            )
                        })
                        .and_then(
                            move |(connection, results): (SharedConnection, Vec<bool>)| {
                                if let Some(index) = results.iter().position(|val| *val) {
                                    warn!("An account already exists with the same {}. Cannot insert account: {:?}", keys[index], account);
                                    Err(())
                                } else {
                                    Ok((connection, account))
                                }
                            },
                        )
                })
                .and_then(move |(connection, account)| {
                    let mut pipe = redis::pipe();
                    pipe.atomic();

                    // Set account details
                    pipe.cmd("HMSET").arg(account_details_key(account.id)).arg(account.clone().encrypt_tokens(&encryption_key))
                        .ignore();

                    // Set balance-related details
                    pipe.hset_multiple(account_details_key(account.id), &[("balance", 0), ("prepaid_amount", 0)]).ignore();

                    // Set incoming auth details
                    if let Some(auth) = btp_incoming_token_hmac_clone {
                        pipe.hset("btp_auth", auth.as_ref(), account.id).ignore();
                    }

                    if let Some(auth) = http_incoming_token_hmac_clone {
                        pipe.hset("http_auth", auth.as_ref(), account.id).ignore();
                    }

                    // Add settlement details
                    if let Some(ref xrp_address) = account.xrp_address {
                        pipe.hset("xrp_addresses", xrp_address, account.id).ignore();
                    }

                    if account.send_routes {
                        pipe.sadd("send_routes_to", account.id).ignore();
                    }

                    // Add route to routing table
                    pipe.hset(ROUTES_KEY, account.ilp_address.to_vec(), account.id)
                        .ignore();

                    pipe.query_async(connection)
                        .map_err(|err| error!("Error inserting account into DB: {:?}", err))
                        .and_then(move |(connection, _ret): (SharedConnection, Value)| {
                            update_routes(connection, routing_table)
                        })
                        .and_then(move |_| Ok(account))
                }),
        )
    }
}

impl AccountStore for RedisStore {
    type Account = Account;

    // TODO cache results to avoid hitting Redis for each packet
    fn get_accounts(
        &self,
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
    ) -> Box<Future<Item = Vec<Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        let num_accounts = account_ids.len();
        let mut pipe = redis::pipe();
        for account_id in account_ids.iter() {
            pipe.hgetall(account_details_key(*account_id));
        }
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error querying details for accounts: {:?} {:?}",
                        account_ids, err
                    )
                })
                .and_then(
                    move |(_conn, accounts): (_, Vec<AccountWithEncryptedTokens>)| {
                        if accounts.len() == num_accounts {
                            let accounts = accounts
                                .into_iter()
                                .map(|account| account.decrypt_tokens(&decryption_key))
                                .collect();
                            Ok(accounts)
                        } else {
                            Err(())
                        }
                    },
                ),
        )
    }
}

impl BalanceStore for RedisStore {
    /// Returns the balance **from the account holder's perspective**, meaning the sum of
    /// the Payable Balance and Pending Outgoing minus the Receivable Balance and the Pending Incoming.
    fn get_balance(&self, account: Account) -> Box<Future<Item = i64, Error = ()> + Send> {
        Box::new(
            cmd("HMGET")
                .arg(account_details_key(account.id))
                .arg(&["balance", "prepaid_amount"])
                .query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error getting balance for account: {} {:?}",
                        account.id, err
                    )
                })
                .and_then(|(_connection, values): (_, Vec<i64>)| {
                    let balance = values[0];
                    let prepaid_amount = values[1];
                    Ok(balance + prepaid_amount)
                }),
        )
    }

    fn update_balances_for_prepare(
        &self,
        from_account: Account,
        incoming_amount: u64,
        to_account: Account,
        _outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        if incoming_amount > 0 {
            let from_account_id = from_account.id;
            let to_account_id = to_account.id;
            Box::new(
                cmd("EVAL")
                    .arg(PROCESS_PREPARE)
                    .arg(0)
                    .arg(from_account_id)
                    .arg(incoming_amount)
                    .query_async(self.connection.as_ref().clone())
                    .map_err(move |err| {
                        warn!(
                            "Error handling prepare from account: {} to account: {}: {:?}",
                            from_account_id, to_account_id, err
                        )
                    })
                    .and_then(move |(_connection, balance): (_, i64)| {
                        debug!(
                            "Processed prepare. Account {} has balance (including prepaid amount): {} ",
                            from_account_id, balance
                        );
                        Ok(())
                    }),
            )
        } else {
            Box::new(ok(()))
        }
    }

    fn update_balances_for_fulfill(
        &self,
        from_account: Account,
        _incoming_amount: u64,
        to_account: Account,
        outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        if outgoing_amount > 0 {
            let from_account_id = from_account.id;
            let to_account_id = to_account.id;
            Box::new(
                cmd("EVAL")
                    .arg(PROCESS_FULFILL)
                    .arg(0)
                    .arg(to_account_id)
                    .arg(outgoing_amount)
                    .query_async(self.connection.as_ref().clone())
                    .map_err(move |err| {
                        error!(
                            "Error handling fulfill from account: {} to account: {}: {:?}",
                            from_account_id, to_account_id, err
                        )
                    })
                    .and_then(move |(_connection, balance): (_, i64)| {
                        debug!(
                            "Processed fulfill. Account {} has balance: {}",
                            to_account_id, balance,
                        );
                        Ok(())
                    }),
            )
        } else {
            Box::new(ok(()))
        }
    }

    fn update_balances_for_reject(
        &self,
        from_account: Account,
        incoming_amount: u64,
        to_account: Account,
        _outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        if incoming_amount > 0 {
            let from_account_id = from_account.id;
            let to_account_id = to_account.id;
            Box::new(
                cmd("EVAL")
                    .arg(PROCESS_REJECT)
                    .arg(0)
                    .arg(from_account_id)
                    .arg(incoming_amount)
                    .query_async(self.connection.as_ref().clone())
                    .map_err(move |err| {
                        warn!(
                            "Error handling reject for packet from account: {} to account: {}: {:?}",
                            from_account_id, to_account_id, err
                        )
                    })
                    .and_then(move |(_connection, balance): (_, i64)| {
                        debug!(
                            "Processed reject. Account {} has balance (including prepaid amount): {}",
                            from_account_id, balance
                        );
                        Ok(())
                    }),
            )
        } else {
            Box::new(ok(()))
        }
    }
}

impl ExchangeRateStore for RedisStore {
    fn get_exchange_rates(&self, asset_codes: &[&str]) -> Result<Vec<f64>, ()> {
        let rates: Vec<f64> = asset_codes
            .iter()
            .filter_map(|code| {
                (*self.exchange_rates.read())
                    .get(&code.to_string())
                    .cloned()
            })
            .collect();
        if rates.len() == asset_codes.len() {
            Ok(rates)
        } else {
            Err(())
        }
    }
}

impl BtpStore for RedisStore {
    type Account = Account;

    fn get_account_from_btp_token(
        &self,
        token: &str,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send> {
        // TODO make sure it can't do script injection!
        // TODO cache the result so we don't hit redis for every packet (is that necessary if redis is often used as a cache?)
        let decryption_key = self.decryption_key.clone();
        Box::new(
            cmd("EVAL")
                .arg(ACCOUNT_FROM_INDEX)
                .arg(1)
                .arg("btp_auth")
                .arg(hmac::sign(&self.hmac_key, token.as_bytes()).as_ref())
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error getting account from BTP token: {:?}", err))
                .and_then(
                    move |(_connection, account): (_, Option<AccountWithEncryptedTokens>)| {
                        if let Some(account) = account {
                            let account = account.decrypt_tokens(&decryption_key);
                            Ok(account)
                        } else {
                            warn!("No account found with BTP token");
                            Err(())
                        }
                    },
                ),
        )
    }
}

impl HttpStore for RedisStore {
    type Account = Account;

    fn get_account_from_http_token(
        &self,
        token: &str,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send> {
        // TODO make sure it can't do script injection!
        let decryption_key = self.decryption_key.clone();
        Box::new(
            cmd("EVAL")
                .arg(ACCOUNT_FROM_INDEX)
                .arg(1)
                .arg("http_auth")
                .arg(hmac::sign(&self.hmac_key, token.as_bytes()).as_ref())
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error getting account from HTTP auth: {:?}", err))
                .and_then(
                    move |(_connection, account): (_, Option<AccountWithEncryptedTokens>)| {
                        if let Some(account) = account {
                            let account = account.decrypt_tokens(&decryption_key);
                            Ok(account)
                        } else {
                            warn!("No account found with given HTTP auth");
                            Err(())
                        }
                    },
                ),
        )
    }
}

impl RouterStore for RedisStore {
    fn routing_table(&self) -> HashMap<Bytes, u64> {
        self.routes.read().clone()
    }
}

impl NodeStore for RedisStore {
    type Account = Account;

    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<Future<Item = Account, Error = ()> + Send> {
        self.create_new_account(account)
    }

    // TODO limit the number of results and page through them
    fn get_all_accounts(&self) -> Box<Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        Box::new(
            cmd("GET")
                .arg(NEXT_ACCOUNT_ID_KEY)
                .query_async(self.connection.as_ref().clone())
                .and_then(
                    move |(connection, next_account_id): (SharedConnection, u64)| {
                        let mut pipe = redis::pipe();
                        for i in 0..next_account_id {
                            pipe.hgetall(account_details_key(i));
                        }
                        pipe.query_async(connection).and_then(
                            move |(_, accounts): (_, Vec<AccountWithEncryptedTokens>)| {
                                let accounts: Vec<Account> = accounts
                                    .into_iter()
                                    .map(|account| account.decrypt_tokens(&decryption_key))
                                    .collect();
                                Ok(accounts)
                            },
                        )
                    },
                )
                .map_err(|err| error!("Error getting all accounts: {:?}", err)),
        )
    }

    fn set_rates<R>(&self, rates: R) -> Box<Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, f64)>,
    {
        let rates: Vec<(String, f64)> = rates.into_iter().collect();
        let exchange_rates = self.exchange_rates.clone();
        let mut pipe = redis::pipe();
        pipe.atomic()
            .del(RATES_KEY)
            .ignore()
            .hset_multiple(RATES_KEY, &rates)
            .ignore();
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error setting rates: {:?}", err))
                .and_then(move |(connection, _): (SharedConnection, Value)| {
                    update_rates(connection, exchange_rates)
                }),
        )
    }

    // TODO fix inconsistency betwen this method and set_routes which
    // takes the prefixes as Bytes and the account as an Account object
    fn set_static_routes<R>(&self, routes: R) -> Box<Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, u64)>,
    {
        let routes: Vec<(String, u64)> = routes.into_iter().collect();
        let accounts: HashSet<u64> =
            HashSet::from_iter(routes.iter().map(|(_prefix, account_id)| *account_id));
        let mut pipe = redis::pipe();
        for account_id in accounts {
            pipe.exists(account_details_key(account_id));
        }

        let routing_table = self.routes.clone();
        Box::new(pipe.query_async(self.connection.as_ref().clone())
            .map_err(|err| error!("Error checking if accounts exist while setting static routes: {:?}", err))
            .and_then(|(connection, accounts_exist): (SharedConnection, Vec<bool>)| {
                if accounts_exist.iter().all(|a| *a) {
                    Ok(connection)
                } else {
                    error!("Error setting static routes because not all of the given accounts exist");
                    Err(())
                }
            })
            .and_then(move |connection| {
        let mut pipe = redis::pipe();
        pipe.atomic()
            .del(STATIC_ROUTES_KEY)
            .ignore()
            .hset_multiple(STATIC_ROUTES_KEY, &routes)
            .ignore();
            pipe.query_async(connection)
                .map_err(|err| error!("Error setting static routes: {:?}", err))
                .and_then(move |(connection, _): (SharedConnection, Value)| {
                    update_routes(connection, routing_table)
                })
            }))
    }

    fn set_static_route(
        &self,
        prefix: String,
        account_id: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        let routing_table = self.routes.clone();
        let prefix_clone = prefix.clone();
        Box::new(
        cmd("EXISTS")
            .arg(account_details_key(account_id))
            .query_async(self.connection.as_ref().clone())
            .map_err(|err| error!("Error checking if account exists before setting static route: {:?}", err))
            .and_then(move |(connection, exists): (SharedConnection, bool)| {
                if exists {
                    Ok(connection)
                } else {
                    error!("Cannot set static route for prefix: {} because account {} does not exist", prefix_clone, account_id);
                    Err(())
                }
            })
            .and_then(move |connection| {
                cmd("HSET")
                    .arg(STATIC_ROUTES_KEY)
                    .arg(prefix)
                    .arg(account_id)
                    .query_async(connection)
                    .map_err(|err| error!("Error setting static route: {:?}", err))
                    .and_then(move |(connection, _): (SharedConnection, Value)| {
                        update_routes(connection, routing_table)
                    })
            })
        )
    }
}

impl RouteManagerStore for RedisStore {
    type Account = Account;

    fn get_accounts_to_send_routes_to(
        &self,
    ) -> Box<Future<Item = Vec<Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        Box::new(
            cmd("SMEMBERS")
                .arg("send_routes_to")
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error getting members of set send_routes_to: {:?}", err))
                .and_then(|(connection, account_ids): (SharedConnection, Vec<u64>)| {
                    if account_ids.is_empty() {
                        Either::A(ok(Vec::new()))
                    } else {
                        let mut pipe = redis::pipe();
                        for id in account_ids {
                            pipe.hgetall(account_details_key(id));
                        }
                        Either::B(
                            pipe.query_async(connection)
                                .map_err(|err| {
                                    error!("Error getting accounts to send routes to: {:?}", err)
                                })
                                .and_then(
                                    move |(_connection, accounts): (
                                        SharedConnection,
                                        Vec<AccountWithEncryptedTokens>,
                                    )| {
                                        let accounts: Vec<Account> = accounts
                                            .into_iter()
                                            .map(|account| account.decrypt_tokens(&decryption_key))
                                            .collect();
                                        Ok(accounts)
                                    },
                                ),
                        )
                    }
                }),
        )
    }

    fn get_local_and_configured_routes(
        &self,
    ) -> Box<Future<Item = ((HashMap<Bytes, Account>), (HashMap<Bytes, Account>)), Error = ()> + Send>
    {
        let get_static_routes = cmd("HGETALL")
            .arg(STATIC_ROUTES_KEY)
            .query_async(self.connection.as_ref().clone())
            .map_err(|err| error!("Error getting static routes: {:?}", err))
            .and_then(
                |(_, static_routes): (SharedConnection, Vec<(String, u64)>)| Ok(static_routes),
            );
        Box::new(self.get_all_accounts().join(get_static_routes).and_then(
            |(accounts, static_routes)| {
                let local_table = HashMap::from_iter(
                    accounts
                        .iter()
                        .map(|account| (account.ilp_address.clone(), account.clone())),
                );

                let account_map: HashMap<u64, &Account> = HashMap::from_iter(accounts.iter().map(|account| (account.id, account)));
                let configured_table: HashMap<Bytes, Account> = HashMap::from_iter(static_routes.into_iter()
                    .filter_map(|(prefix, account_id)| {
                        if let Some(account) = account_map.get(&account_id) {
                            Some((Bytes::from(prefix), (*account).clone()))
                        } else {
                            warn!("No account for ID: {}, ignoring configured route for prefix: {}", account_id, prefix);
                            None
                        }
                    }));

                Ok((local_table, configured_table))
            },
        ))
    }

    fn set_routes<R>(&mut self, routes: R) -> Box<Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (Bytes, Account)>,
    {
        let routes: Vec<(String, u64)> = routes
            .into_iter()
            .filter_map(|(prefix, account)| {
                if let Ok(prefix) = String::from_utf8(prefix.to_vec()) {
                    Some((prefix, account.id))
                } else {
                    None
                }
            })
            .collect();
        let num_routes = routes.len();

        // Save routes to Redis
        let routing_tale = self.routes.clone();
        let mut pipe = redis::pipe();
        pipe.atomic()
            .del(ROUTES_KEY)
            .ignore()
            .hset_multiple(ROUTES_KEY, &routes)
            .ignore();
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error setting routes: {:?}", err))
                .and_then(move |(connection, _): (SharedConnection, Value)| {
                    trace!("Saved {} routes to Redis", num_routes);
                    update_routes(connection, routing_tale)
                }),
        )
    }
}

impl RateLimitStore for RedisStore {
    type Account = Account;

    /// Apply rate limits for number of packets per minute and amount of money per minute
    ///
    /// This uses https://github.com/brandur/redis-cell so the redis-cell module MUST be loaded into redis before this is run
    fn apply_rate_limits(
        &self,
        account: Account,
        prepare_amount: u64,
    ) -> Box<Future<Item = (), Error = RateLimitError> + Send> {
        if account.amount_per_minute_limit.is_some() || account.packets_per_minute_limit.is_some() {
            let mut pipe = redis::pipe();
            let packet_limit = account.packets_per_minute_limit.is_some();
            let amount_limit = account.amount_per_minute_limit.is_some();

            if let Some(limit) = account.packets_per_minute_limit {
                let limit = limit - 1;
                pipe.cmd("CL.THROTTLE")
                    .arg(format!("limit:packets:{}", account.id))
                    .arg(limit)
                    .arg(limit)
                    .arg(60)
                    .arg(1);
            }

            if let Some(limit) = account.amount_per_minute_limit {
                let limit = limit - 1;
                pipe.cmd("CL.THROTTLE")
                    .arg(format!("limit:throughput:{}", account.id))
                    // TODO allow separate configuration for burst limit
                    .arg(limit)
                    .arg(limit)
                    .arg(60)
                    .arg(prepare_amount);
            }
            Box::new(
                pipe.query_async(self.connection.as_ref().clone())
                    .map_err(|err| {
                        error!("Error applying rate limits: {:?}", err);
                        RateLimitError::StoreError
                    })
                    .and_then(move |(_, results): (_, Vec<Vec<i64>>)| {
                        if packet_limit && amount_limit {
                            if results[0][0] == 1 {
                                Err(RateLimitError::PacketLimitExceeded)
                            } else if results[1][0] == 1 {
                                Err(RateLimitError::ThroughputLimitExceeded)
                            } else {
                                Ok(())
                            }
                        } else if packet_limit && results[0][0] == 1 {
                            Err(RateLimitError::PacketLimitExceeded)
                        } else if amount_limit && results[0][0] == 1 {
                            Err(RateLimitError::ThroughputLimitExceeded)
                        } else {
                            Ok(())
                        }
                    }),
            )
        } else {
            Box::new(ok(()))
        }
    }

    fn refund_throughput_limit(
        &self,
        account: Account,
        prepare_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        if let Some(limit) = account.amount_per_minute_limit {
            let limit = limit - 1;
            Box::new(
                cmd("CL.THROTTLE")
                    .arg(format!("limit:throughput:{}", account.id))
                    .arg(limit)
                    .arg(limit)
                    .arg(60)
                    // TODO make sure this doesn't overflow
                    .arg(0i64 - (prepare_amount as i64))
                    .query_async(self.connection.as_ref().clone())
                    .map_err(|err| error!("Error refunding throughput limit: {:?}", err))
                    .and_then(|(_, _): (_, Value)| Ok(())),
            )
        } else {
            Box::new(ok(()))
        }
    }
}

// TODO replace this with pubsub when async pubsub is added upstream: https://github.com/mitsuhiko/redis-rs/issues/183
fn update_rates(
    connection: SharedConnection,
    exchange_rates: Arc<RwLock<HashMap<String, f64>>>,
) -> impl Future<Item = (), Error = ()> {
    cmd("HGETALL")
        .arg(RATES_KEY)
        .query_async(connection)
        .map_err(|err| error!("Error polling for exchange rates: {:?}", err))
        .and_then(move |(_connection, rates): (_, Vec<(String, f64)>)| {
            let num_assets = rates.len();
            let rates = HashMap::from_iter(rates.into_iter());
            (*exchange_rates.write()) = rates;
            debug!("Updated rates for {} assets", num_assets);
            Ok(())
        })
}

// TODO replace this with pubsub when async pubsub is added upstream: https://github.com/mitsuhiko/redis-rs/issues/183
type RouteVec = Vec<(String, u64)>;

fn update_routes(
    connection: SharedConnection,
    routing_table: Arc<RwLock<HashMap<Bytes, u64>>>,
) -> impl Future<Item = (), Error = ()> {
    let mut pipe = redis::pipe();
    pipe.hgetall(ROUTES_KEY).hgetall(STATIC_ROUTES_KEY);
    pipe.query_async(connection)
        .map_err(|err| error!("Error polling for routing table updates: {:?}", err))
        .and_then(
            move |(_connection, (routes, static_routes)): (_, (RouteVec, RouteVec))| {
                trace!(
                    "Loaded routes from redis. Static routes: {:?}, other routes: {:?}",
                    static_routes,
                    routes
                );
                let routes = HashMap::from_iter(
                    routes
                        .into_iter()
                        // Having the static_routes inserted after ensures that they will overwrite
                        // any routes with the same prefix from the first set
                        .chain(static_routes.into_iter())
                        .map(|(prefix, account_id)| (Bytes::from(prefix), account_id)),
                );
                trace!("Routing table is now: {:?}", routes);
                let num_routes = routes.len();
                *routing_table.write() = routes;
                debug!("Updated routing table with {} routes", num_routes);
                Ok(())
            },
        )
}
