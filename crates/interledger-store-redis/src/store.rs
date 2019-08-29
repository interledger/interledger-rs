// The informal schema of our data in redis:
//   send_routes_to         set         used for CCP routing
//   receive_routes_from    set         used for CCP routing
//   next_account_id        string      unique ID for each new account
//   rates:current          hash        exchange rates
//   routes:current         hash        dynamic routing table
//   routes:static          hash        static routing table
//   accounts:<id>          hash        information for each account
//   btp_outgoing
// For interactive exploration of the store,
// use the redis-cli tool included with your redis install.
// Within redis-cli:
//    keys *                list all keys of any type in the store
//    smembers <key>        list the members of a set
//    get <key>             get the value of a key
//    hgetall <key>         the flattened list of every key/value entry within a hash

use super::account::*;
use super::crypto::generate_keys;
use bytes::Bytes;
use futures::{
    future::{err, ok, result, Either},
    Future, Stream,
};
use log::{debug, error, trace, warn};
use std::collections::{HashMap, HashSet};

use super::account::AccountId;
use http::StatusCode;
use interledger_api::{AccountDetails, NodeStore};
use interledger_btp::BtpStore;
use interledger_ccp::RouteManagerStore;
use interledger_http::HttpStore;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore, RateLimitError, RateLimitStore};
use interledger_settlement::{IdempotentData, IdempotentStore, SettlementStore};
use lazy_static::lazy_static;
use parking_lot::RwLock;
use redis::{
    self, aio::SharedConnection, cmd, Client, ConnectionInfo, PipelineCommands, Script, Value,
};
use ring::aead;
use std::{
    iter::FromIterator,
    str,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_executor::spawn;
use tokio_timer::Interval;

const DEFAULT_POLL_INTERVAL: u64 = 30000; // 30 seconds

// The following are Lua scripts that are used to atomically execute the given logic
// inside Redis. This allows for more complex logic without needing multiple round
// trips for messages to be sent to and from Redis, as well as locks to ensure no other
// process is accessing Redis at the same time.
// For more information on scripting in Redis, see https://redis.io/commands/eval
lazy_static! {
    // This lua script receives the token type (HTTP/BTP), the username and a user
    // provided auth token. It fetches the account id associated with that username.
    // Then, it fetches the token associated with that account id. Finally, it
    // compares the fetched token with the one provided by the user, and returns the
    // account that corresponds to the fetched account id if they match.
    static ref ACCOUNT_FROM_TOKEN: Script = Script::new("
    local token_type = ARGV[1]
    local acc_id = redis.call('HGET', 'usernames', ARGV[2])
    local provided_token = ARGV[3]

    local id_key = 'accounts:' .. acc_id
    local token = redis.call('HGET', id_key, token_type)
    if token ~= provided_token then
        return nil
    end
    return redis.call('HGETALL', id_key)");

    static ref PROCESS_PREPARE: Script = Script::new("
    local from_id = ARGV[1]
    local from_account = 'accounts:' .. ARGV[1]
    local from_amount = tonumber(ARGV[2])
    local min_balance, balance, prepaid_amount = unpack(redis.call('HMGET', from_account, 'min_balance', 'balance', 'prepaid_amount'))
    balance = tonumber(balance)
    prepaid_amount = tonumber(prepaid_amount)

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

    return balance + prepaid_amount");

    static ref PROCESS_FULFILL: Script = Script::new("
    local to_account = 'accounts:' .. ARGV[1]
    local to_amount = tonumber(ARGV[2])

    local balance = redis.call('HINCRBY', to_account, 'balance', to_amount)
    local prepaid_amount, settle_threshold, settle_to = unpack(redis.call('HMGET', to_account, 'prepaid_amount', 'settle_threshold', 'settle_to'))

    -- The logic for trigerring settlement is as follows:
    --  1. settle_threshold must be non-nil (if it's nil, then settlement was perhaps disabled on the account).
    --  2. balance must be greater than settle_threshold (this is the core of the 'should I settle logic')
    --  3. settle_threshold must be greater than settle_to (e.g., settleTo=5, settleThreshold=6)
    local settle_amount = 0
    if (settle_threshold and settle_to) and (balance >= tonumber(settle_threshold)) and (tonumber(settle_threshold) > tonumber(settle_to)) then
        settle_amount = balance - tonumber(settle_to)

        -- Update the balance _before_ sending the settlement so that we don't accidentally send
        -- multiple settlements for the same balance. If the settlement fails we'll roll back
        -- the balance change by re-adding the amount back to the balance
        balance = settle_to
        redis.call('HSET', to_account, 'balance', balance)
    end

    return {balance + prepaid_amount, settle_amount}");

    static ref PROCESS_REJECT: Script = Script::new("
    local from_account = 'accounts:' .. ARGV[1]
    local from_amount = tonumber(ARGV[2])

    local prepaid_amount = redis.call('HGET', from_account, 'prepaid_amount')
    local balance = redis.call('HINCRBY', from_account, 'balance', from_amount)
    return balance + prepaid_amount");

    static ref REFUND_SETTLEMENT: Script = Script::new("
    local account = 'accounts:' .. ARGV[1]
    local settle_amount = tonumber(ARGV[2])

    local balance = redis.call('HINCRBY', account, 'balance', settle_amount)
    return balance");

    static ref PROCESS_INCOMING_SETTLEMENT: Script = Script::new("
    local account = 'accounts:' .. ARGV[1]
    local amount = tonumber(ARGV[2])
    local idempotency_key = ARGV[3]

    local balance, prepaid_amount = unpack(redis.call('HMGET', account, 'balance', 'prepaid_amount'))

    -- If idempotency key has been used, then do not perform any operations
    if redis.call('EXISTS', idempotency_key) == 1 then
        return balance + prepaid_amount
    end

    -- Otherwise, set it to true and make it expire after 24h (86400 sec)
    redis.call('SET', idempotency_key, 'true', 'EX', 86400)

    -- Credit the incoming settlement to the balance and/or prepaid amount,
    -- depending on whether that account currently owes money or not
    if tonumber(balance) >= 0 then
        prepaid_amount = redis.call('HINCRBY', account, 'prepaid_amount', amount)
    elseif math.abs(balance) >= amount then
        balance = redis.call('HINCRBY', account, 'balance', amount)
    else
        prepaid_amount = redis.call('HINCRBY', account, 'prepaid_amount', amount + balance)
        balance = 0
        redis.call('HSET', account, 'balance', 0)
    end

    return balance + prepaid_amount");
}

static ROUTES_KEY: &str = "routes:current";
static RATES_KEY: &str = "rates:current";
static STATIC_ROUTES_KEY: &str = "routes:static";

fn prefixed_idempotency_key(idempotency_key: String) -> String {
    format!("idempotency-key:{}", idempotency_key)
}

fn accounts_key(account_id: AccountId) -> String {
    format!("accounts:{}", account_id)
}

pub struct RedisStoreBuilder {
    redis_uri: ConnectionInfo,
    secret: [u8; 32],
    poll_interval: u64,
}

impl RedisStoreBuilder {
    pub fn new(redis_uri: ConnectionInfo, secret: [u8; 32]) -> Self {
        RedisStoreBuilder {
            redis_uri,
            secret,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    pub fn poll_interval(&mut self, poll_interval: u64) -> &mut Self {
        self.poll_interval = poll_interval;
        self
    }

    pub fn connect(&self) -> impl Future<Item = RedisStore, Error = ()> {
        let (encryption_key, decryption_key) = generate_keys(&self.secret[..]);
        let poll_interval = self.poll_interval;

        result(Client::open(self.redis_uri.clone()))
            .map_err(|err| error!("Error creating Redis client: {:?}", err))
            .and_then(|client| {
                debug!("Connected to redis: {:?}", client);
                client
                    .get_shared_async_connection()
                    .map_err(|err| error!("Error connecting to Redis: {:?}", err))
            })
            .and_then(move |connection| {
                let store = RedisStore {
                    connection: Arc::new(connection),
                    exchange_rates: Arc::new(RwLock::new(HashMap::new())),
                    routes: Arc::new(RwLock::new(HashMap::new())),
                    encryption_key: Arc::new(encryption_key),
                    decryption_key: Arc::new(decryption_key),
                };

                // Start polling for rate updates
                // Note: if this behavior changes, make sure to update the Drop implementation
                let connection_clone = Arc::downgrade(&store.connection);
                let exchange_rates = store.exchange_rates.clone();
                let poll_rates =
                    Interval::new(Instant::now(), Duration::from_millis(poll_interval))
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
                let poll_routes =
                    Interval::new(Instant::now(), Duration::from_millis(poll_interval))
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
    routes: Arc<RwLock<HashMap<Bytes, AccountId>>>,
    encryption_key: Arc<aead::SealingKey>,
    decryption_key: Arc<aead::OpeningKey>,
}

impl RedisStore {
    pub fn get_all_accounts_ids(&self) -> impl Future<Item = Vec<AccountId>, Error = ()> {
        let mut pipe = redis::pipe();
        pipe.smembers("accounts");
        pipe.query_async(self.connection.as_ref().clone())
            .map_err(|err| error!("Error getting account IDs: {:?}", err))
            .and_then(|(_conn, account_ids): (_, Vec<Vec<AccountId>>)| Ok(account_ids[0].clone()))
    }

    fn redis_insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        let connection = self.connection.clone();
        let routing_table = self.routes.clone();
        let encryption_key = self.encryption_key.clone();

        let id = AccountId::new();
        debug!("Generated account: {}", id);
        Box::new(
            result(Account::try_from(id, account))
                .and_then(move |account| {
                    // Check that there isn't already an account with values that MUST be unique
                    let mut pipe = redis::pipe();
                    pipe.exists(accounts_key(account.id));
                    pipe.hexists("usernames", account.username().as_ref());

                    pipe.query_async(connection.as_ref().clone())
                        .map_err(|err| {
                            error!("Error checking whether account details already exist: {:?}", err)
                        })
                        .and_then(
                            move |(connection, results): (SharedConnection, Vec<bool>)| {
                                if results.iter().any(|val| *val) {
                                    warn!("An account already exists with the same {}. Cannot insert account: {:?}", id, account);
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

                    // Add the account key to the list of accounts
                    pipe.sadd("accounts", id).ignore();

                    // Save map for Username -> Account ID
                    pipe.hset("usernames", account.username().as_ref(), id).ignore();

                    // Set account details
                    pipe.cmd("HMSET").arg(accounts_key(account.id)).arg(account.clone().encrypt_tokens(&encryption_key))
                        .ignore();

                    // Set balance-related details
                    pipe.hset_multiple(accounts_key(account.id), &[("balance", 0), ("prepaid_amount", 0)]).ignore();

                    if account.send_routes {
                        pipe.sadd("send_routes_to", account.id).ignore();
                    }

                    if account.receive_routes {
                        pipe.sadd("receive_routes_from", account.id).ignore();
                    }

                    if account.btp_uri.is_some() {
                        pipe.sadd("btp_outgoing", account.id).ignore();
                    }

                    // Add route to routing table
                    pipe.hset(ROUTES_KEY, account.ilp_address.to_bytes().to_vec(), account.id)
                        .ignore();

                    pipe.query_async(connection)
                        .map_err(|err| error!("Error inserting account into DB: {:?}", err))
                        .and_then(move |(connection, _ret): (SharedConnection, Value)| {
                            update_routes(connection, routing_table)
                        })
                        .and_then(move |_| {
                            debug!("Inserted account {} (ILP address: {})", account.id, str::from_utf8(account.ilp_address.as_ref()).unwrap_or("<not utf8>"));
                            Ok(account)
                        })
                })
        )
    }

    fn redis_update_account(
        &self,
        id: AccountId,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        let connection = self.connection.clone();
        let routing_table = self.routes.clone();
        let encryption_key = self.encryption_key.clone();

        Box::new(
            // Check to make sure an account with this ID already exists
            redis::cmd("EXISTS")
                .arg(accounts_key(id))
                // TODO this needs to be atomic with the insertions later, waiting on #186
                .query_async(connection.as_ref().clone())
                .map_err(|err| error!("Error checking whether ID exists: {:?}", err))
                .and_then(move |(connection, result): (SharedConnection, bool)| {
                    if result {
                        Account::try_from(id, account)
                            .and_then(move |account| Ok((connection, account)))
                    } else {
                        warn!(
                            "No account exists with ID {}, cannot update account {:?}",
                            id, account
                        );
                        Err(())
                    }
                })
                .and_then(move |(connection, account)| {
                    let mut pipe = redis::pipe();
                    pipe.atomic();

                    // Add the account key to the list of accounts
                    pipe.sadd("accounts", id).ignore();

                    // Set account details
                    pipe.cmd("HMSET")
                        .arg(accounts_key(account.id))
                        .arg(account.clone().encrypt_tokens(&encryption_key))
                        .ignore();

                    if account.send_routes {
                        pipe.sadd("send_routes_to", account.id).ignore();
                    }

                    if account.receive_routes {
                        pipe.sadd("receive_routes_from", account.id).ignore();
                    }

                    if account.btp_uri.is_some() {
                        pipe.sadd("btp_outgoing", account.id).ignore();
                    }

                    // Add route to routing table
                    pipe.hset(
                        ROUTES_KEY,
                        account.ilp_address.to_bytes().to_vec(),
                        account.id,
                    )
                    .ignore();

                    pipe.query_async(connection)
                        .map_err(|err| error!("Error inserting account into DB: {:?}", err))
                        .and_then(move |(connection, _ret): (SharedConnection, Value)| {
                            update_routes(connection, routing_table)
                        })
                        .and_then(move |_| {
                            debug!(
                                "Inserted account {} (ILP address: {})",
                                account.id,
                                str::from_utf8(account.ilp_address.as_ref())
                                    .unwrap_or("<not utf8>")
                            );
                            Ok(account)
                        })
                }),
        )
    }

    fn redis_delete_account(
        &self,
        id: AccountId,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        let connection = self.connection.as_ref().clone();
        let routing_table = self.routes.clone();

        Box::new(
            // TODO: a get_account API to avoid making Vecs which we only need one element of
            self.redis_get_accounts(vec![id])
                .and_then(|accounts| accounts.get(0).cloned().ok_or(()))
                .and_then(|account| {
                    let mut pipe = redis::pipe();
                    pipe.atomic();

                    pipe.srem("accounts", account.id).ignore();

                    pipe.del(accounts_key(account.id)).ignore();
                    pipe.hdel("usernames", account.username().as_ref()).ignore();

                    if account.send_routes {
                        pipe.srem("send_routes_to", account.id).ignore();
                    }

                    if account.receive_routes {
                        pipe.srem("receive_routes_from", account.id).ignore();
                    }

                    if account.btp_uri.is_some() {
                        pipe.srem("btp_outgoing", account.id).ignore();
                    }

                    pipe.hdel(ROUTES_KEY, account.ilp_address.to_bytes().to_vec())
                        .ignore();

                    pipe.query_async(connection)
                        .map_err(|err| error!("Error deleting account from DB: {:?}", err))
                        .and_then(move |(connection, _ret): (SharedConnection, Value)| {
                            update_routes(connection, routing_table)
                        })
                        .and_then(move |_| {
                            debug!("Deleted account {}", account.id);
                            Ok(account)
                        })
                }),
        )
    }

    fn redis_get_accounts(
        &self,
        account_ids: Vec<AccountId>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        let num_accounts = account_ids.len();
        let mut pipe = redis::pipe();
        for account_id in account_ids.iter() {
            pipe.hgetall(accounts_key(*account_id));
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

impl AccountStore for RedisStore {
    type Account = Account;

    // TODO cache results to avoid hitting Redis for each packet
    fn get_accounts(
        &self,
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        self.redis_get_accounts(account_ids)
    }

    fn get_account_id_from_username(
        &self,
        username: &Username,
    ) -> Box<dyn Future<Item = AccountId, Error = ()> + Send> {
        Box::new(
            cmd("HGET")
                .arg("usernames")
                .arg(username.as_ref())
                .query_async(self.connection.as_ref().clone())
                .map_err(move |err| error!("Error getting account id: {:?}", err))
                .and_then(|(_connection, id): (_, AccountId)| Ok(id)),
        )
    }
}

impl BalanceStore for RedisStore {
    /// Returns the balance **from the account holder's perspective**, meaning the sum of
    /// the Payable Balance and Pending Outgoing minus the Receivable Balance and the Pending Incoming.
    fn get_balance(&self, account: Account) -> Box<dyn Future<Item = i64, Error = ()> + Send> {
        Box::new(
            cmd("HMGET")
                .arg(accounts_key(account.id))
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
        from_account: Account, // TODO: Make this take only the id
        incoming_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        if incoming_amount > 0 {
            let from_account_id = from_account.id;
            Box::new(
                PROCESS_PREPARE
                    .arg(from_account_id)
                    .arg(incoming_amount)
                    .invoke_async(self.connection.as_ref().clone())
                    .map_err(move |err| {
                        warn!(
                            "Error handling prepare from account: {}:  {:?}",
                            from_account_id, err
                        )
                    })
                    .and_then(move |(_connection, balance): (_, i64)| {
                        trace!(
                            "Processed prepare with incoming amount: {}. Account {} has balance (including prepaid amount): {} ",
                            incoming_amount, from_account_id, balance
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
        to_account: Account, // TODO: Make this take only the id
        outgoing_amount: u64,
    ) -> Box<dyn Future<Item = (i64, u64), Error = ()> + Send> {
        if outgoing_amount > 0 {
            debug!(
                "To: {}, Amount paid: {}",
                to_account.ilp_address, outgoing_amount
            );
            let to_account_id = to_account.id;
            Box::new(
                PROCESS_FULFILL
                    .arg(to_account_id)
                    .arg(outgoing_amount)
                    .invoke_async(self.connection.as_ref().clone())
                    .map_err(move |err| {
                        error!(
                            "Error handling Fulfill received from account: {}: {:?}",
                            to_account_id, err
                        )
                    })
                    .and_then(move |(_connection, (balance, amount_to_settle)): (_, (i64, u64))| {
                        trace!("Processed fulfill for account {} for outgoing amount {}. Fulfill call result: {} {}",
                            to_account_id,
                            outgoing_amount,
                            balance,
                            amount_to_settle,
                        );
                        Ok((balance, amount_to_settle))
                    })
            )
        } else {
            Box::new(ok((0, 0)))
        }
    }

    fn update_balances_for_reject(
        &self,
        from_account: Account, // TODO: Make this take only the id
        incoming_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        if incoming_amount > 0 {
            let from_account_id = from_account.id;
            Box::new(
                PROCESS_REJECT
                    .arg(from_account_id)
                    .arg(incoming_amount)
                    .invoke_async(self.connection.as_ref().clone())
                    .map_err(move |err| {
                        warn!(
                            "Error handling reject for packet from account: {}: {:?}",
                            from_account_id, err
                        )
                    })
                    .and_then(move |(_connection, balance): (_, i64)| {
                        trace!(
                            "Processed reject for incoming amount: {}. Account {} has balance (including prepaid amount): {}",
                            incoming_amount, from_account_id, balance
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

    fn get_account_from_btp_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        // TODO make sure it can't do script injection!
        // TODO cache the result so we don't hit redis for every packet (is that necessary if redis is often used as a cache?)
        let decryption_key = self.decryption_key.clone();
        Box::new(
            ACCOUNT_FROM_TOKEN
                .arg("btp_incoming_token")
                .arg(username.as_ref())
                .arg(token)
                .invoke_async(self.connection.as_ref().clone())
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

    fn get_btp_outgoing_accounts(
        &self,
    ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        Box::new(
            cmd("SMEMBERS")
                .arg("btp_outgoing")
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error getting members of set btp_outgoing: {:?}", err))
                .and_then(
                    |(connection, account_ids): (SharedConnection, Vec<AccountId>)| {
                        if account_ids.is_empty() {
                            Either::A(ok(Vec::new()))
                        } else {
                            let mut pipe = redis::pipe();
                            for id in account_ids {
                                pipe.hgetall(accounts_key(id));
                            }
                            Either::B(
                                pipe.query_async(connection)
                                    .map_err(|err| {
                                        error!(
                                        "Error getting accounts with outgoing BTP details: {:?}",
                                        err
                                    )
                                    })
                                    .and_then(
                                        move |(_connection, accounts): (
                                            SharedConnection,
                                            Vec<AccountWithEncryptedTokens>,
                                        )| {
                                            let accounts: Vec<Account> = accounts
                                                .into_iter()
                                                .map(|account| {
                                                    account.decrypt_tokens(&decryption_key)
                                                })
                                                .collect();
                                            Ok(accounts)
                                        },
                                    ),
                            )
                        }
                    },
                ),
        )
    }
}

impl HttpStore for RedisStore {
    type Account = Account;

    /// Checks if the stored token for the provided account id matches the
    /// provided token, and if so, returns the account associated with that token
    fn get_account_from_http_auth(
        &self,
        username: &Username,
        token: &str,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        // TODO make sure it can't do script injection!
        let decryption_key = self.decryption_key.clone();
        Box::new(
            ACCOUNT_FROM_TOKEN
                .arg("http_incoming_token")
                .arg(username.as_ref())
                .arg(token)
                .invoke_async(self.connection.as_ref().clone())
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
    fn routing_table(&self) -> HashMap<Bytes, <Self::Account as AccountTrait>::AccountId> {
        self.routes.read().clone()
    }
}

impl NodeStore for RedisStore {
    type Account = Account;

    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        self.redis_insert_account(account)
    }

    fn delete_account(&self, id: AccountId) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        self.redis_delete_account(id)
    }

    fn update_account(
        &self,
        id: AccountId,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        self.redis_update_account(id, account)
    }

    // TODO limit the number of results and page through them
    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        let mut pipe = redis::pipe();
        let connection = self.connection.clone();
        pipe.smembers("accounts");
        Box::new(self.get_all_accounts_ids().and_then(move |account_ids| {
            let mut pipe = redis::pipe();
            for account_id in account_ids {
                pipe.hgetall(accounts_key(account_id));
            }

            pipe.query_async(connection.as_ref().clone())
                .map_err(|err| error!("Error getting account ids: {:?}", err))
                .and_then(
                    move |(_, accounts): (_, Vec<Option<AccountWithEncryptedTokens>>)| {
                        let accounts: Vec<Account> = accounts
                            .into_iter()
                            .filter_map(|a| a)
                            .map(|account| account.decrypt_tokens(&decryption_key))
                            .collect();
                        Ok(accounts)
                    },
                )
        }))
    }

    fn set_rates<R>(&self, rates: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
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
                    trace!("Set exchange rates: {:?}", exchange_rates);
                    update_rates(connection, exchange_rates)
                }),
        )
    }

    // TODO fix inconsistency betwen this method and set_routes which
    // takes the prefixes as Bytes and the account as an Account object
    fn set_static_routes<R>(&self, routes: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, AccountId)>,
    {
        let routes: Vec<(String, AccountId)> = routes.into_iter().collect();
        let accounts: HashSet<_> =
            HashSet::from_iter(routes.iter().map(|(_prefix, account_id)| account_id));
        let mut pipe = redis::pipe();
        for account_id in accounts {
            pipe.exists(accounts_key(*account_id));
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
        account_id: AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let routing_table = self.routes.clone();
        let prefix_clone = prefix.clone();
        Box::new(
        cmd("EXISTS")
            .arg(accounts_key(account_id))
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

type RoutingTable<A> = HashMap<Bytes, A>;

impl RouteManagerStore for RedisStore {
    type Account = Account;

    fn get_accounts_to_send_routes_to(
        &self,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        Box::new(
            cmd("SMEMBERS")
                .arg("send_routes_to")
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error getting members of set send_routes_to: {:?}", err))
                .and_then(
                    |(connection, account_ids): (SharedConnection, Vec<AccountId>)| {
                        if account_ids.is_empty() {
                            Either::A(ok(Vec::new()))
                        } else {
                            let mut pipe = redis::pipe();
                            for id in account_ids {
                                pipe.hgetall(accounts_key(id));
                            }
                            Either::B(
                                pipe.query_async(connection)
                                    .map_err(|err| {
                                        error!(
                                            "Error getting accounts to send routes to: {:?}",
                                            err
                                        )
                                    })
                                    .and_then(
                                        move |(_connection, accounts): (
                                            SharedConnection,
                                            Vec<AccountWithEncryptedTokens>,
                                        )| {
                                            let accounts: Vec<Account> = accounts
                                                .into_iter()
                                                .map(|account| {
                                                    account.decrypt_tokens(&decryption_key)
                                                })
                                                .collect();
                                            Ok(accounts)
                                        },
                                    ),
                            )
                        }
                    },
                ),
        )
    }

    fn get_accounts_to_receive_routes_from(
        &self,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        Box::new(
            cmd("SMEMBERS")
                .arg("receive_routes_from")
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| {
                    error!(
                        "Error getting members of set receive_routes_from: {:?}",
                        err
                    )
                })
                .and_then(
                    |(connection, account_ids): (SharedConnection, Vec<AccountId>)| {
                        if account_ids.is_empty() {
                            Either::A(ok(Vec::new()))
                        } else {
                            let mut pipe = redis::pipe();
                            for id in account_ids {
                                pipe.hgetall(accounts_key(id));
                            }
                            Either::B(
                                pipe.query_async(connection)
                                    .map_err(|err| {
                                        error!(
                                            "Error getting accounts to receive routes from: {:?}",
                                            err
                                        )
                                    })
                                    .and_then(
                                        move |(_connection, accounts): (
                                            SharedConnection,
                                            Vec<AccountWithEncryptedTokens>,
                                        )| {
                                            let accounts: Vec<Account> = accounts
                                                .into_iter()
                                                .map(|account| {
                                                    account.decrypt_tokens(&decryption_key)
                                                })
                                                .collect();
                                            Ok(accounts)
                                        },
                                    ),
                            )
                        }
                    },
                ),
        )
    }

    fn get_local_and_configured_routes(
        &self,
    ) -> Box<dyn Future<Item = (RoutingTable<Account>, RoutingTable<Account>), Error = ()> + Send>
    {
        let get_static_routes = cmd("HGETALL")
            .arg(STATIC_ROUTES_KEY)
            .query_async(self.connection.as_ref().clone())
            .map_err(|err| error!("Error getting static routes: {:?}", err))
            .and_then(
                |(_, static_routes): (SharedConnection, Vec<(String, AccountId)>)| {
                    Ok(static_routes)
                },
            );
        Box::new(self.get_all_accounts().join(get_static_routes).and_then(
            |(accounts, static_routes)| {
                let local_table = HashMap::from_iter(
                    accounts
                        .iter()
                        .map(|account| (account.ilp_address.to_bytes(), account.clone())),
                );

                let account_map: HashMap<AccountId, &Account> = HashMap::from_iter(accounts.iter().map(|account| (account.id, account)));
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

    fn set_routes(
        &mut self,
        routes: impl IntoIterator<Item = (Bytes, Account)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let routes: Vec<(String, AccountId)> = routes
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
    ) -> Box<dyn Future<Item = (), Error = RateLimitError> + Send> {
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
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
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

impl IdempotentStore for RedisStore {
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentData>, Error = ()> + Send> {
        let idempotency_key_clone = idempotency_key.clone();
        Box::new(
            cmd("HGETALL")
                .arg(prefixed_idempotency_key(idempotency_key.clone()))
                .query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error loading idempotency key {}: {:?}",
                        idempotency_key_clone, err
                    )
                })
                .and_then(move |(_connection, ret): (_, HashMap<String, String>)| {
                    if let (Some(status_code), Some(data), Some(input_hash_slice)) = (
                        ret.get("status_code"),
                        ret.get("data"),
                        ret.get("input_hash"),
                    ) {
                        trace!("Loaded idempotency key {:?} - {:?}", idempotency_key, ret);
                        let mut input_hash: [u8; 32] = Default::default();
                        input_hash.copy_from_slice(input_hash_slice.as_ref());
                        Ok(Some((
                            StatusCode::from_str(status_code).unwrap(),
                            Bytes::from(data.clone()),
                            input_hash,
                        )))
                    } else {
                        Ok(None)
                    }
                }),
        )
    }

    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("HMSET") // cannot use hset_multiple since data and status_code have different types
            .arg(&prefixed_idempotency_key(idempotency_key.clone()))
            .arg("status_code")
            .arg(status_code.as_u16())
            .arg("data")
            .arg(data.as_ref())
            .arg("input_hash")
            .arg(&input_hash)
            .ignore()
            .expire(&prefixed_idempotency_key(idempotency_key.clone()), 86400)
            .ignore();
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error caching: {:?}", err))
                .and_then(move |(_connection, _): (_, Vec<String>)| {
                    trace!(
                        "Cached {:?}: {:?}, {:?}",
                        idempotency_key,
                        status_code,
                        data,
                    );
                    Ok(())
                }),
        )
    }
}

impl SettlementStore for RedisStore {
    type Account = Account;

    fn update_balance_for_incoming_settlement(
        &self,
        account_id: AccountId,
        amount: u64,
        idempotency_key: Option<String>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let idempotency_key = idempotency_key.unwrap();
        Box::new(
            PROCESS_INCOMING_SETTLEMENT
            .arg(account_id)
            .arg(amount)
            .arg(idempotency_key)
            .invoke_async(self.connection.as_ref().clone())
            .map_err(move |err| error!("Error processing incoming settlement from account: {} for amount: {}: {:?}", account_id, amount, err))
            .and_then(move |(_connection, balance): (_, i64)| {
                trace!("Processed incoming settlement from account: {} for amount: {}. Balance is now: {}", account_id, amount, balance);
                Ok(())
            }))
    }

    fn refund_settlement(
        &self,
        account_id: AccountId,
        settle_amount: u64,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        trace!(
            "Refunding settlement for account: {} of amount: {}",
            account_id,
            settle_amount
        );
        Box::new(
            REFUND_SETTLEMENT
                .arg(account_id)
                .arg(settle_amount)
                .invoke_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error refunding settlement for account: {} of amount: {}: {:?}",
                        account_id, settle_amount, err
                    )
                })
                .and_then(move |(_connection, balance): (_, i64)| {
                    trace!(
                        "Refunded settlement for account: {} of amount: {}. Balance is now: {}",
                        account_id,
                        settle_amount,
                        balance
                    );
                    Ok(())
                }),
        )
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
            trace!("Updated rates for {} assets", num_assets);
            Ok(())
        })
}

// TODO replace this with pubsub when async pubsub is added upstream: https://github.com/mitsuhiko/redis-rs/issues/183
type RouteVec = Vec<(String, AccountId)>;

fn update_routes(
    connection: SharedConnection,
    routing_table: Arc<RwLock<HashMap<Bytes, AccountId>>>,
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
                trace!("Routing table is: {:?}", routes);
                *routing_table.write() = routes;
                Ok(())
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future;
    use redis::IntoConnectionInfo;
    use tokio::runtime::Runtime;

    #[test]
    fn connect_fails_if_db_unavailable() {
        let mut runtime = Runtime::new().unwrap();
        runtime
            .block_on(future::lazy(
                || -> Box<dyn Future<Item = (), Error = ()> + Send> {
                    Box::new(
                        RedisStoreBuilder::new(
                            "redis://127.0.0.1:0".into_connection_info().unwrap() as ConnectionInfo,
                            [0; 32],
                        )
                        .connect()
                        .then(|result| {
                            assert!(result.is_err());
                            Ok(())
                        }),
                    )
                },
            ))
            .unwrap();
    }
}
