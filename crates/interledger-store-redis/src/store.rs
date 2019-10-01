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
use super::crypto::{encrypt_token, generate_keys, DecryptionKey, EncryptionKey};
use super::reconnect::RedisReconnect;
use bytes::Bytes;
use futures::{
    future::{err, ok, result, Either},
    sync::mpsc::UnboundedSender,
    Future, Stream,
};
use log::{debug, error, trace, warn};
use std::collections::{HashMap, HashSet};

use super::account::AccountId;
use http::StatusCode;
use interledger_api::{AccountDetails, AccountSettings, EncryptedAccountSettings, NodeStore};
use interledger_btp::BtpStore;
use interledger_ccp::{CcpRoutingAccount, RouteManagerStore, RoutingRelation};
use interledger_http::HttpStore;
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore, AddressStore, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore, RateLimitError, RateLimitStore};
use interledger_settlement::{
    scale_with_precision_loss, Convert, ConvertDetails, IdempotentData, IdempotentStore,
    LeftoversStore, SettlementStore,
};
use interledger_stream::{PaymentNotification, StreamNotificationsStore};
use lazy_static::lazy_static;
use num_bigint::BigUint;
use parking_lot::RwLock;
use redis::{
    self, aio::SharedConnection, cmd, Client, ConnectionInfo, ControlFlow, ErrorKind,
    FromRedisValue, PipelineCommands, PubSubCommands, RedisError, RedisWrite, Script, ToRedisArgs,
    Value,
};
use secrecy::{ExposeSecret, Secret};
use serde_json;
use std::{
    iter::{self, FromIterator},
    str,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_executor::spawn;
use tokio_timer::Interval;
use url::Url;
use zeroize::Zeroize;

const DEFAULT_POLL_INTERVAL: u64 = 30000; // 30 seconds

static PARENT_ILP_KEY: &str = "parent_node_account_address";
static ROUTES_KEY: &str = "routes:current";
static STATIC_ROUTES_KEY: &str = "routes:static";
static DEFAULT_ROUTE_KEY: &str = "routes:default";
static STREAM_NOTIFICATIONS_PREFIX: &str = "stream_notifications:";
static SETTLEMENT_ENGINES_KEY: &str = "settlement_engines";

fn uncredited_amount_key(account_id: impl ToString) -> String {
    format!("uncredited-amount:{}", account_id.to_string())
}

fn prefixed_idempotency_key(idempotency_key: String) -> String {
    format!("idempotency-key:{}", idempotency_key)
}

fn accounts_key(account_id: AccountId) -> String {
    format!("accounts:{}", account_id)
}

// The following are Lua scripts that are used to atomically execute the given logic
// inside Redis. This allows for more complex logic without needing multiple round
// trips for messages to be sent to and from Redis, as well as locks to ensure no other
// process is accessing Redis at the same time.
// For more information on scripting in Redis, see https://redis.io/commands/eval
lazy_static! {
    static ref DEFAULT_ILP_ADDRESS: Address = Address::from_str("local.host").unwrap();

    /// This lua script fetches an account associated with a username. The client
    /// MUST ensure that the returned account is authenticated.
    static ref ACCOUNT_FROM_USERNAME: Script = Script::new("
    local username = ARGV[1]
    if redis.call('HEXISTS', 'usernames', username) then
        local id = redis.call('HGET', 'usernames', ARGV[1])
        return redis.call('HGETALL', 'accounts:' .. id)
    else
        return nil
    end");

    /// Load a list of accounts
    /// If an account does not have a settlement_engine_url set
    /// but there is one configured for that account's currency,
    /// it will use the globally configured url
    static ref LOAD_ACCOUNTS: Script = Script::new("
    -- borrowed from https://stackoverflow.com/a/34313599
    local function into_dictionary(flat_map)
        local result = {}
        for i = 1, #flat_map, 2 do
            result[flat_map[i]] = flat_map[i + 1]
        end
        return result
    end

    local settlement_engines = into_dictionary(redis.call('HGETALL', 'settlement_engines'))
    local accounts = {}

    -- TODO get rid of the two representations of account
    -- For some reason, the result from HGETALL returns
    -- a bulk value type that we can return but that
    -- we cannot index into with string keys. In contrast,
    -- the result from into_dictionary can be indexed into
    -- but if we try to return it, redis thinks it is a
    -- '(empty list or set)'. There _should_ be some better way to do
    -- this simple operation and a less janky way to insert the
    -- settlement_engine_url into the account we are going to return
    local account
    local account_dict
    for index, id in ipairs(ARGV) do
        account = redis.call('HGETALL', 'accounts:' .. id)

        if account ~= nil then
            account_dict = into_dictionary(account)

            -- If the account does not have a settlement_engine_url specified
            -- but there is one configured for that currency, set the
            -- account to use that url
            if account_dict.settlement_engine_url == nil then
                local url = settlement_engines[account_dict.asset_code]
                if url ~= nil then
                    table.insert(account, 'settlement_engine_url')
                    table.insert(account, url)
                end
            end

            table.insert(accounts, account)
        end
    end
    return accounts");

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

pub struct RedisStoreBuilder {
    redis_url: ConnectionInfo,
    secret: [u8; 32],
    poll_interval: u64,
    node_ilp_address: Address,
}

impl RedisStoreBuilder {
    pub fn new(redis_url: ConnectionInfo, secret: [u8; 32]) -> Self {
        RedisStoreBuilder {
            redis_url,
            secret,
            poll_interval: DEFAULT_POLL_INTERVAL,
            node_ilp_address: DEFAULT_ILP_ADDRESS.clone(),
        }
    }

    pub fn node_ilp_address(&mut self, node_ilp_address: Address) -> &mut Self {
        self.node_ilp_address = node_ilp_address;
        self
    }

    pub fn poll_interval(&mut self, poll_interval: u64) -> &mut Self {
        self.poll_interval = poll_interval;
        self
    }

    pub fn connect(&mut self) -> impl Future<Item = RedisStore, Error = ()> {
        let redis_info = self.redis_url.clone();
        let (encryption_key, decryption_key) = generate_keys(&self.secret[..]);
        self.secret.zeroize(); // clear the secret after it has been used for key generation
        let poll_interval = self.poll_interval;
        let ilp_address = self.node_ilp_address.clone();

        RedisReconnect::connect(redis_info.clone())
            .map_err(|_| ())
            .join(
                result(Client::open(redis_info.clone()))
                    .map_err(|err| error!("Error creating subscription Redis client: {:?}", err))
                    .and_then(|client| {
                        debug!("Connected subscription client to redis: {:?}", client);
                        client.get_connection().map_err(|err| {
                            error!("Error connecting subscription client to Redis: {:?}", err)
                        })
                    }),
            )
            .and_then(move |(connection, mut sub_connection)| {
                // Before initializing the store, check if we have an address
                // that was configured due to adding a parent. If no parent was
                // found, use the builder's provided address (local.host) or the
                // one we decided to override it with
                redis::cmd("GET")
                    .arg(PARENT_ILP_KEY)
                    .query_async(connection.clone())
                    .map_err(|err| {
                        error!(
                            "Error checking whether we have a parent configured: {:?}",
                            err
                        )
                    })
                    .and_then(move |(_, address): (RedisReconnect, Option<String>)| {
                        Ok(if let Some(address) = address {
                            Address::from_str(&address).unwrap()
                        } else {
                            ilp_address
                        })
                    })
                    .and_then(move |node_ilp_address| {
                        let store = RedisStore {
                            ilp_address: Arc::new(RwLock::new(node_ilp_address)),
                            connection,
                            subscriptions: Arc::new(RwLock::new(HashMap::new())),
                            exchange_rates: Arc::new(RwLock::new(HashMap::new())),
                            routes: Arc::new(RwLock::new(HashMap::new())),
                            encryption_key: Arc::new(encryption_key),
                            decryption_key: Arc::new(decryption_key),
                        };

                        // Poll for routing table updates
                        // Note: if this behavior changes, make sure to update the Drop implementation
                        let connection_clone = Arc::downgrade(&store.connection.conn);
                        let redis_info = store.connection.redis_info.clone();
                        let routing_table = store.routes.clone();
                        let poll_routes =
                            Interval::new(Instant::now(), Duration::from_millis(poll_interval))
                                .map_err(|err| error!("Interval error: {:?}", err))
                                .for_each(move |_| {
                                    if let Some(conn) = connection_clone.upgrade() {
                                        Either::A(update_routes(
                                            RedisReconnect {
                                                conn,
                                                redis_info: redis_info.clone(),
                                            },
                                            routing_table.clone(),
                                        ))
                                    } else {
                                        debug!("Not polling routes anymore because connection was closed");
                                        // TODO make sure the interval stops
                                        Either::B(err(()))
                                    }
                                });
                        spawn(poll_routes);

                        // Here we spawn a worker thread to listen for incoming messages on Redis pub/sub,
                        // running a callback for each message received.
                        // This currently must be a thread rather than a task due to the redis-rs driver
                        // not yet supporting asynchronous subscriptions (see https://github.com/mitsuhiko/redis-rs/issues/183).
                        let subscriptions_clone = store.subscriptions.clone();
                        std::thread::spawn(move || {
                            let sub_status =
                                sub_connection.psubscribe::<_, _, Vec<String>>(&["*"], move |msg| {
                                    let channel_name = msg.get_channel_name();
                                    if channel_name.starts_with(STREAM_NOTIFICATIONS_PREFIX) {
                                        if let Ok(account_id) = AccountId::from_str(&channel_name[STREAM_NOTIFICATIONS_PREFIX.len()..]) {
                                            let message: PaymentNotification = match serde_json::from_slice(msg.get_payload_bytes()) {
                                                Ok(s) => s,
                                                Err(e) => {
                                                    error!("Failed to get payload from subscription: {}", e);
                                                    return ControlFlow::Continue;
                                                }
                                            };
                                            trace!("Subscribed message received for account {}: {:?}", account_id, message);
                                            match subscriptions_clone.read().get(&account_id) {
                                                Some(sender) => {
                                                    if let Err(err) = sender.unbounded_send(message) {
                                                        error!("Failed to send message: {}", err);
                                                    }
                                                }
                                                None => trace!("Ignoring message for account {} because there were no open subscriptions", account_id),
                                            }
                                        } else {
                                            error!("Invalid AccountId in channel name: {}", channel_name);
                                        }
                                    } else {
                                        warn!("Ignoring unexpected message from Redis subscription for channel: {}", channel_name);
                                    }
                                    ControlFlow::Continue
                                });
                            match sub_status {
                                Err(e) => warn!("Could not issue psubscribe to Redis: {}", e),
                                Ok(_) => debug!("Successfully subscribed to Redis pubsub"),
                            }
                        });

                Ok(store)
            })
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
    pub ilp_address: Arc<RwLock<Address>>,
    connection: RedisReconnect,
    subscriptions: Arc<RwLock<HashMap<AccountId, UnboundedSender<PaymentNotification>>>>,
    exchange_rates: Arc<RwLock<HashMap<String, f64>>>,
    routes: Arc<RwLock<HashMap<Bytes, AccountId>>>,
    encryption_key: Arc<Secret<EncryptionKey>>,
    decryption_key: Arc<Secret<DecryptionKey>>,
}

impl RedisStore {
    pub fn get_all_accounts_ids(&self) -> impl Future<Item = Vec<AccountId>, Error = ()> {
        let mut pipe = redis::pipe();
        pipe.smembers("accounts");
        pipe.query_async(self.connection.clone())
            .map_err(|err| error!("Error getting account IDs: {:?}", err))
            .and_then(|(_conn, account_ids): (_, Vec<Vec<AccountId>>)| Ok(account_ids[0].clone()))
    }

    fn redis_insert_account(
        &self,
        encrypted: AccountWithEncryptedTokens,
    ) -> Box<dyn Future<Item = AccountWithEncryptedTokens, Error = ()> + Send> {
        let account = encrypted.account.clone();
        let ret = encrypted.clone();
        let connection = self.connection.clone();
        let routing_table = self.routes.clone();
        // Check that there isn't already an account with values that MUST be unique
        let mut pipe = redis::pipe();
        pipe.exists(accounts_key(account.id));
        pipe.hexists("usernames", account.username().as_ref());
        if account.routing_relation == RoutingRelation::Parent {
            pipe.exists(PARENT_ILP_KEY);
        }

        Box::new(pipe.query_async(connection.clone())
            .map_err(|err| {
                error!("Error checking whether account details already exist: {:?}", err)
            })
            .and_then(
                move |(connection, results): (RedisReconnect, Vec<bool>)| {
                    if results.iter().any(|val| *val) {
                        warn!("An account already exists with the same {}. Cannot insert account: {:?}", account.id, account);
                        Err(())
                    } else {
                        Ok((connection, account))
                    }
            })
            .and_then(move |(connection, account)| {
                let mut pipe = redis::pipe();
                pipe.atomic();

                // Add the account key to the list of accounts
                pipe.sadd("accounts", account.id).ignore();

                // Save map for Username -> Account ID
                pipe.hset("usernames", account.username().as_ref(), account.id).ignore();

                // Set account details
                pipe.cmd("HMSET")
                    .arg(accounts_key(account.id))
                    .arg(encrypted).ignore();

                // Set balance-related details
                pipe.hset_multiple(accounts_key(account.id), &[("balance", 0), ("prepaid_amount", 0)]).ignore();

                if account.should_send_routes() {
                    pipe.sadd("send_routes_to", account.id).ignore();
                }

                if account.should_receive_routes() {
                    pipe.sadd("receive_routes_from", account.id).ignore();
                }

                if account.ilp_over_btp_url.is_some() {
                    pipe.sadd("btp_outgoing", account.id).ignore();
                }

                // Add route to routing table
                pipe.hset(ROUTES_KEY, account.ilp_address.to_bytes().to_vec(), account.id)
                    .ignore();

                // The parent account settings are done via the API. We just
                // had to check for the existence of a parent
                pipe.query_async(connection)
                    .map_err(|err| error!("Error inserting account into DB: {:?}", err))
                    .and_then(move |(connection, _ret): (RedisReconnect, Value)| {
                        update_routes(connection, routing_table)
                    })
                    .and_then(move |_| {
                        debug!("Inserted account {} (ILP address: {})", account.id, account.ilp_address);
                        Ok(ret)
                    })
            }))
    }

    fn redis_update_account(
        &self,
        encrypted: AccountWithEncryptedTokens,
    ) -> Box<dyn Future<Item = AccountWithEncryptedTokens, Error = ()> + Send> {
        let account = encrypted.account.clone();
        let connection = self.connection.clone();
        let routing_table = self.routes.clone();
        Box::new(
            // Check to make sure an account with this ID already exists
            redis::cmd("EXISTS")
                .arg(accounts_key(account.id))
                // TODO this needs to be atomic with the insertions later,
                // waiting on #186
                // TODO: Do not allow this update to happen if
                // AccountDetails.RoutingRelation == Parent and parent is
                // already set
                .query_async(connection.clone())
                .map_err(|err| error!("Error checking whether ID exists: {:?}", err))
                .and_then(move |(connection, exists): (RedisReconnect, bool)| {
                    if !exists {
                        warn!(
                            "No account exists with ID {}, cannot update account {:?}",
                            account.id, account
                        );
                        return Either::A(err(()));
                    }
                    let mut pipe = redis::pipe();
                    pipe.atomic();

                    // Add the account key to the list of accounts
                    pipe.sadd("accounts", account.id).ignore();

                    // Set account details
                    pipe.cmd("HMSET")
                        .arg(accounts_key(account.id))
                        .arg(encrypted.clone())
                        .ignore();

                    if account.should_send_routes() {
                        pipe.sadd("send_routes_to", account.id).ignore();
                    }

                    if account.should_receive_routes() {
                        pipe.sadd("receive_routes_from", account.id).ignore();
                    }

                    if account.ilp_over_btp_url.is_some() {
                        pipe.sadd("btp_outgoing", account.id).ignore();
                    }

                    // Add route to routing table
                    pipe.hset(
                        ROUTES_KEY,
                        account.ilp_address.to_bytes().to_vec(),
                        account.id,
                    )
                    .ignore();

                    Either::B(
                        pipe.query_async(connection)
                            .map_err(|err| error!("Error inserting account into DB: {:?}", err))
                            .and_then(move |(connection, _ret): (RedisReconnect, Value)| {
                                update_routes(connection, routing_table)
                            })
                            .and_then(move |_| {
                                debug!(
                                    "Inserted account {} (id: {}, ILP address: {})",
                                    account.username, account.id, account.ilp_address
                                );
                                Ok(encrypted)
                            }),
                    )
                }),
        )
    }

    fn redis_modify_account(
        &self,
        id: AccountId,
        settings: EncryptedAccountSettings,
    ) -> Box<dyn Future<Item = AccountWithEncryptedTokens, Error = ()> + Send> {
        let connection = self.connection.clone();
        let self_clone = self.clone();

        let mut pipe = redis::pipe();
        pipe.atomic();

        if let Some(ref endpoint) = settings.ilp_over_btp_url {
            pipe.hset(accounts_key(id), "ilp_over_btp_url", endpoint);
        }

        if let Some(ref endpoint) = settings.ilp_over_http_url {
            pipe.hset(accounts_key(id), "ilp_over_http_url", endpoint);
        }

        if let Some(ref token) = settings.ilp_over_btp_outgoing_token {
            pipe.hset(
                accounts_key(id),
                "ilp_over_btp_outgoing_token",
                token.as_ref(),
            );
        }

        if let Some(ref token) = settings.ilp_over_http_outgoing_token {
            pipe.hset(
                accounts_key(id),
                "ilp_over_http_outgoing_token",
                token.as_ref(),
            );
        }

        if let Some(ref token) = settings.ilp_over_btp_incoming_token {
            pipe.hset(
                accounts_key(id),
                "ilp_over_btp_incoming_token",
                token.as_ref(),
            );
        }

        if let Some(ref token) = settings.ilp_over_http_incoming_token {
            pipe.hset(
                accounts_key(id),
                "ilp_over_http_incoming_token",
                token.as_ref(),
            );
        }

        if let Some(settle_threshold) = settings.settle_threshold {
            pipe.hset(accounts_key(id), "settle_threshold", settle_threshold);
        }

        if let Some(settle_to) = settings.settle_to {
            pipe.hset(accounts_key(id), "settle_to", settle_to);
        }

        Box::new(
            pipe.query_async(connection.clone())
                .map_err(|err| error!("Error modifying user account: {:?}", err))
                .and_then(move |(_connection, _ret): (RedisReconnect, Value)| {
                    // return the updated account
                    self_clone.redis_get_account(id)
                }),
        )
    }

    fn redis_get_account(
        &self,
        id: AccountId,
    ) -> Box<dyn Future<Item = AccountWithEncryptedTokens, Error = ()> + Send> {
        Box::new(
            LOAD_ACCOUNTS
                .arg(id.to_string())
                .invoke_async(self.connection.get_shared_connection())
                .map_err(|err| error!("Error loading accounts: {:?}", err))
                .and_then(|(_, mut accounts): (_, Vec<AccountWithEncryptedTokens>)| {
                    accounts.pop().ok_or(())
                }),
        )
    }

    fn redis_delete_account(
        &self,
        id: AccountId,
    ) -> Box<dyn Future<Item = AccountWithEncryptedTokens, Error = ()> + Send> {
        let connection = self.connection.clone();
        let routing_table = self.routes.clone();
        Box::new(self.redis_get_account(id).and_then(move |encrypted| {
            let account = encrypted.account.clone();
            let mut pipe = redis::pipe();
            pipe.atomic();

            pipe.srem("accounts", account.id).ignore();

            pipe.del(accounts_key(account.id)).ignore();
            pipe.hdel("usernames", account.username().as_ref()).ignore();

            if account.should_send_routes() {
                pipe.srem("send_routes_to", account.id).ignore();
            }

            if account.should_receive_routes() {
                pipe.srem("receive_routes_from", account.id).ignore();
            }

            if account.ilp_over_btp_url.is_some() {
                pipe.srem("btp_outgoing", account.id).ignore();
            }

            pipe.hdel(ROUTES_KEY, account.ilp_address.to_bytes().to_vec())
                .ignore();

            pipe.query_async(connection)
                .map_err(|err| error!("Error deleting account from DB: {:?}", err))
                .and_then(move |(connection, _ret): (RedisReconnect, Value)| {
                    update_routes(connection, routing_table)
                })
                .and_then(move |_| {
                    debug!("Deleted account {}", account.id);
                    Ok(encrypted)
                })
        }))
    }
}

impl AccountStore for RedisStore {
    type Account = Account;

    // TODO cache results to avoid hitting Redis for each packet
    fn get_accounts(
        &self,
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        let num_accounts = account_ids.len();
        let mut script = LOAD_ACCOUNTS.prepare_invoke();
        for id in account_ids.iter() {
            script.arg(id.to_string());
        }
        Box::new(
            script
                .invoke_async(self.connection.get_shared_connection())
                .map_err(|err| error!("Error loading accounts: {:?}", err))
                .and_then(move |(_, accounts): (_, Vec<AccountWithEncryptedTokens>)| {
                    if accounts.len() == num_accounts {
                        let accounts = accounts
                            .into_iter()
                            .map(|account| {
                                account.decrypt_tokens(&decryption_key.expose_secret().0)
                            })
                            .collect();
                        Ok(accounts)
                    } else {
                        Err(())
                    }
                }),
        )
    }

    fn get_account_id_from_username(
        &self,
        username: &Username,
    ) -> Box<dyn Future<Item = AccountId, Error = ()> + Send> {
        let username = username.clone();
        Box::new(
            cmd("HGET")
                .arg("usernames")
                .arg(username.as_ref())
                .query_async(self.connection.clone())
                .map_err(move |err| error!("Error getting account id: {:?}", err))
                .and_then(|(_connection, id): (_, Option<AccountId>)| {
                    id.ok_or_else(move || debug!("Username not found: {}", username))
                }),
        )
    }
}

impl StreamNotificationsStore for RedisStore {
    type Account = Account;

    fn add_payment_notification_subscription(
        &self,
        id: AccountId,
        sender: UnboundedSender<PaymentNotification>,
    ) {
        trace!("Added payment notification listener for {}", id);
        self.subscriptions.write().insert(id, sender);
    }

    fn publish_payment_notification(&self, payment: PaymentNotification) {
        let username = payment.to_username.clone();
        let message = serde_json::to_string(&payment).unwrap();
        let connection = self.connection.clone();
        spawn(
            self.get_account_id_from_username(&username)
                .map_err(move |_| {
                    error!(
                        "Failed to find account ID corresponding to username: {}",
                        username
                    )
                })
                .and_then(move |account_id| {
                    debug!(
                        "Publishing payment notification {} for account {}",
                        message, account_id
                    );
                    redis::cmd("PUBLISH")
                        .arg(format!("{}{}", STREAM_NOTIFICATIONS_PREFIX, account_id))
                        .arg(message)
                        .query_async(connection)
                        .map_err(move |err| error!("Error publish message to Redis: {:?}", err))
                        .and_then(move |(_, _): (_, i32)| Ok(()))
                }),
        );
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
                .query_async(self.connection.clone())
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
                    .invoke_async(self.connection.get_shared_connection())
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
            let to_account_id = to_account.id;
            Box::new(
                PROCESS_FULFILL
                    .arg(to_account_id)
                    .arg(outgoing_amount)
                    .invoke_async(self.connection.get_shared_connection())
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
                    .invoke_async(self.connection.get_shared_connection())
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

    fn get_all_exchange_rates(&self) -> Result<HashMap<String, f64>, ()> {
        Ok((*self.exchange_rates.read()).clone())
    }

    fn set_exchange_rates(&self, rates: HashMap<String, f64>) -> Result<(), ()> {
        // TODO publish rate updates through a pubsub mechanism to support horizontally scaling nodes
        (*self.exchange_rates.write()) = rates;
        Ok(())
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
        // TODO cache the result so we don't hit redis for every packet (is that
        // necessary if redis is often used as a cache?)
        let decryption_key = self.decryption_key.clone();
        let token = token.to_owned();
        Box::new(
            ACCOUNT_FROM_USERNAME
                .arg(username.as_ref())
                .invoke_async(self.connection.get_shared_connection())
                .map_err(|err| error!("Error getting account from BTP token: {:?}", err))
                .and_then(
                    move |(_connection, account): (_, Option<AccountWithEncryptedTokens>)| {
                        if let Some(account) = account {
                            let account = account.decrypt_tokens(&decryption_key.expose_secret().0);
                            if let Some(t) = account.ilp_over_btp_incoming_token.clone() {
                                let t = t.expose_secret().clone();
                                if t == Bytes::from(token) {
                                    Ok(account)
                                } else {
                                    debug!(
                                        "Found account {} but BTP auth token was wrong",
                                        account.username
                                    );
                                    Err(())
                                }
                            } else {
                                debug!(
                                    "Account {} does not have an incoming btp token configured",
                                    account.username
                                );
                                Err(())
                            }
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
                .query_async(self.connection.clone())
                .map_err(|err| error!("Error getting members of set btp_outgoing: {:?}", err))
                .and_then(
                    move |(connection, account_ids): (RedisReconnect, Vec<AccountId>)| {
                        if account_ids.is_empty() {
                            Either::A(ok(Vec::new()))
                        } else {
                            let mut script = LOAD_ACCOUNTS.prepare_invoke();
                            for id in account_ids.iter() {
                                script.arg(id.to_string());
                            }
                            Either::B(
                                script
                                    .invoke_async(connection.get_shared_connection())
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
                                                    account.decrypt_tokens(
                                                        &decryption_key.expose_secret().0,
                                                    )
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
        let token = token.to_owned();
        Box::new(
            ACCOUNT_FROM_USERNAME
                .arg(username.as_ref())
                .invoke_async(self.connection.get_shared_connection())
                .map_err(|err| error!("Error getting account from HTTP auth: {:?}", err))
                .and_then(
                    move |(_connection, account): (_, Option<AccountWithEncryptedTokens>)| {
                        if let Some(account) = account {
                            let account = account.decrypt_tokens(&decryption_key.expose_secret().0);
                            if let Some(t) = account.ilp_over_http_incoming_token.clone() {
                                let t = t.expose_secret().clone();
                                if t == Bytes::from(token) {
                                    Ok(account)
                                } else {
                                    Err(())
                                }
                            } else {
                                Err(())
                            }
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
        let encryption_key = self.encryption_key.clone();
        let id = AccountId::new();
        let account = match Account::try_from(id, account, self.get_ilp_address()) {
            Ok(account) => account,
            Err(_) => return Box::new(err(())),
        };
        debug!(
            "Generated account id for {}: {}",
            account.username.clone(),
            account.id
        );
        let encrypted = account
            .clone()
            .encrypt_tokens(&encryption_key.expose_secret().0);
        Box::new(
            self.redis_insert_account(encrypted)
                .and_then(move |_| Ok(account)),
        )
    }

    fn delete_account(&self, id: AccountId) -> Box<dyn Future<Item = Account, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        Box::new(
            self.redis_delete_account(id).and_then(move |account| {
                Ok(account.decrypt_tokens(&decryption_key.expose_secret().0))
            }),
        )
    }

    fn update_account(
        &self,
        id: AccountId,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        let encryption_key = self.encryption_key.clone();
        let decryption_key = self.decryption_key.clone();
        let account = match Account::try_from(id, account, self.get_ilp_address()) {
            Ok(account) => account,
            Err(_) => return Box::new(err(())),
        };
        debug!(
            "Generated account id for {}: {}",
            account.username.clone(),
            account.id
        );
        let encrypted = account
            .clone()
            .encrypt_tokens(&encryption_key.expose_secret().0);
        Box::new(
            self.redis_update_account(encrypted)
                .and_then(move |account| {
                    Ok(account.decrypt_tokens(&decryption_key.expose_secret().0))
                }),
        )
    }

    fn modify_account_settings(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
        settings: AccountSettings,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
        let encryption_key = self.encryption_key.clone();
        let decryption_key = self.decryption_key.clone();
        let settings = EncryptedAccountSettings {
            settle_to: settings.settle_to,
            settle_threshold: settings.settle_threshold,
            ilp_over_btp_url: settings.ilp_over_btp_url,
            ilp_over_http_url: settings.ilp_over_http_url,
            ilp_over_btp_incoming_token: settings.ilp_over_btp_incoming_token.map(|token| {
                encrypt_token(
                    &encryption_key.expose_secret().0,
                    token.expose_secret().as_bytes(),
                )
            }),
            ilp_over_http_incoming_token: settings.ilp_over_http_incoming_token.map(|token| {
                encrypt_token(
                    &encryption_key.expose_secret().0,
                    token.expose_secret().as_bytes(),
                )
            }),
            ilp_over_btp_outgoing_token: settings.ilp_over_btp_outgoing_token.map(|token| {
                encrypt_token(
                    &encryption_key.expose_secret().0,
                    token.expose_secret().as_bytes(),
                )
            }),
            ilp_over_http_outgoing_token: settings.ilp_over_http_outgoing_token.map(|token| {
                encrypt_token(
                    &encryption_key.expose_secret().0,
                    token.expose_secret().as_bytes(),
                )
            }),
        };

        Box::new(
            self.redis_modify_account(id, settings)
                .and_then(move |account| {
                    Ok(account.decrypt_tokens(&decryption_key.expose_secret().0))
                }),
        )
    }

    // TODO limit the number of results and page through them
    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        let mut pipe = redis::pipe();
        let connection = self.connection.clone();
        pipe.smembers("accounts");
        Box::new(self.get_all_accounts_ids().and_then(move |account_ids| {
            let mut script = LOAD_ACCOUNTS.prepare_invoke();
            for id in account_ids.iter() {
                script.arg(id.to_string());
            }
            script
                .invoke_async(connection.get_shared_connection())
                .map_err(|err| error!("Error getting account ids: {:?}", err))
                .and_then(move |(_, accounts): (_, Vec<AccountWithEncryptedTokens>)| {
                    let accounts: Vec<Account> = accounts
                        .into_iter()
                        .map(|account| account.decrypt_tokens(&decryption_key.expose_secret().0))
                        .collect();
                    Ok(accounts)
                })
        }))
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
        Box::new(pipe.query_async(self.connection.clone())
            .map_err(|err| error!("Error checking if accounts exist while setting static routes: {:?}", err))
            .and_then(|(connection, accounts_exist): (RedisReconnect, Vec<bool>)| {
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
                .and_then(move |(connection, _): (RedisReconnect, Value)| {
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
            .query_async(self.connection.clone())
            .map_err(|err| error!("Error checking if account exists before setting static route: {:?}", err))
            .and_then(move |(connection, exists): (RedisReconnect, bool)| {
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
                    .and_then(move |(connection, _): (RedisReconnect, Value)| {
                        update_routes(connection, routing_table)
                    })
            })
        )
    }

    fn set_default_route(
        &self,
        account_id: AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let routing_table = self.routes.clone();
        // TODO replace this with a lua script to do both calls at once
        Box::new(
            cmd("EXISTS")
                .arg(accounts_key(account_id))
                .query_async(self.connection.clone())
                .map_err(|err| {
                    error!(
                        "Error checking if account exists before setting default route: {:?}",
                        err
                    )
                })
                .and_then(move |(connection, exists): (RedisReconnect, bool)| {
                    if exists {
                        Ok(connection)
                    } else {
                        error!(
                            "Cannot set default route because account {} does not exist",
                            account_id
                        );
                        Err(())
                    }
                })
                .and_then(move |connection| {
                    cmd("SET")
                        .arg(DEFAULT_ROUTE_KEY)
                        .arg(account_id)
                        .query_async(connection)
                        .map_err(|err| error!("Error setting default route: {:?}", err))
                        .and_then(move |(connection, _): (RedisReconnect, Value)| {
                            debug!("Set default route to account id: {}", account_id);
                            update_routes(connection, routing_table)
                        })
                }),
        )
    }

    fn set_settlement_engines(
        &self,
        asset_to_url_map: impl IntoIterator<Item = (String, Url)>,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let asset_to_url_map: Vec<(String, String)> = asset_to_url_map
            .into_iter()
            .map(|(asset_code, url)| (asset_code, url.to_string()))
            .collect();
        debug!("Setting settlement engines to {:?}", asset_to_url_map);
        Box::new(
            cmd("HMSET")
                .arg(SETTLEMENT_ENGINES_KEY)
                .arg(asset_to_url_map)
                .query_async(self.connection.clone())
                .map_err(|err| error!("Error setting settlement engines: {:?}", err))
                .and_then(|(_, _): (RedisReconnect, Value)| Ok(())),
        )
    }

    fn get_asset_settlement_engine(
        &self,
        asset_code: &str,
    ) -> Box<dyn Future<Item = Option<Url>, Error = ()> + Send> {
        Box::new(
            cmd("HGET")
                .arg(SETTLEMENT_ENGINES_KEY)
                .arg(asset_code)
                .query_async(self.connection.clone())
                .map_err(|err| error!("Error getting settlement engine: {:?}", err))
                .map(|(_, url): (_, Option<String>)| {
                    if let Some(url) = url {
                        Url::parse(url.as_str())
                            .map_err(|err| {
                                error!(
                                "Settlement engine URL loaded from Redis was not a valid URL: {:?}",
                                err
                            )
                            })
                            .ok()
                    } else {
                        None
                    }
                }),
        )
    }
}

impl AddressStore for RedisStore {
    // Updates the ILP address of the store & iterates over all children and
    // updates their ILP Address to match the new address.
    fn set_ilp_address(
        &self,
        ilp_address: Address,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        debug!("Setting ILP address to: {}", ilp_address);
        let routing_table = self.routes.clone();
        let connection = self.connection.clone();
        let ilp_address_clone = ilp_address.clone();

        // Set the ILP address we have in memory
        (*self.ilp_address.write()) = ilp_address.clone();

        // Save it to Redis
        Box::new(
            cmd("SET")
                .arg(PARENT_ILP_KEY)
                .arg(ilp_address.as_bytes())
                .query_async(self.connection.clone())
                .map_err(|err| error!("Error setting ILP address {:?}", err))
                .and_then(move |(_, _): (RedisReconnect, Value)| Ok(()))
                .join(self.get_all_accounts().and_then(move |accounts| {
                    // TODO: This can be an expensive operation if this function
                    // gets called often. This currently only gets called when
                    // inserting a new parent account in the API. It'd be nice
                    // if we could generate a child's ILP address on the fly,
                    // instead of having to store the username appended to the
                    // node's ilp address. Currently this is not possible, as
                    // account.ilp_address() cannot access any state that exists
                    // on the store.
                    let mut pipe = redis::pipe();
                    for account in accounts {
                        // Update the address and routes of all children and non-routing accounts.
                        if account.routing_relation() != RoutingRelation::Parent
                            && account.routing_relation() != RoutingRelation::Peer
                        {
                            // remove the old route
                            pipe.hdel(ROUTES_KEY, account.ilp_address.as_bytes())
                                .ignore();

                            // if the username of the account ends with the
                            // node's address, we're already configured so no
                            // need to append anything.
                            let ilp_address_clone2 = ilp_address_clone.clone();
                            // Note: We are assuming that if the node's address
                            // ends with the account's username, then this
                            // account represents the node's non routing
                            // account. Is this a reasonable assumption to make?
                            let new_ilp_address =
                                if ilp_address_clone2.segments().rev().next().unwrap()
                                    == account.username().to_string()
                                {
                                    ilp_address_clone2
                                } else {
                                    ilp_address_clone
                                        .with_suffix(account.username().as_bytes())
                                        .unwrap()
                                };
                            pipe.hset(
                                accounts_key(account.id()),
                                "ilp_address",
                                new_ilp_address.as_bytes(),
                            )
                            .ignore();

                            pipe.hset(ROUTES_KEY, new_ilp_address.as_bytes(), account.id())
                                .ignore();
                        }
                    }
                    pipe.query_async(connection.clone())
                        .map_err(|err| error!("Error updating children: {:?}", err))
                        .and_then(move |(connection, _): (RedisReconnect, Value)| {
                            update_routes(connection, routing_table)
                        })
                }))
                .and_then(move |_| Ok(())),
        )
    }

    fn clear_ilp_address(&self) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        let self_clone = self.clone();
        Box::new(
            cmd("DEL")
                .arg(PARENT_ILP_KEY)
                .query_async(self.connection.clone())
                .map_err(|err| error!("Error removing parent address: {:?}", err))
                .and_then(move |(_, _): (RedisReconnect, Value)| {
                    *(self_clone.ilp_address.write()) = DEFAULT_ILP_ADDRESS.clone();
                    Ok(())
                }),
        )
    }

    fn get_ilp_address(&self) -> Address {
        // read consumes the Arc<RwLock<T>> so we cannot return a reference
        self.ilp_address.read().clone()
    }
}

type RoutingTable<A> = HashMap<Bytes, A>;

impl RouteManagerStore for RedisStore {
    type Account = Account;

    fn get_accounts_to_send_routes_to(
        &self,
        ignore_accounts: Vec<AccountId>,
    ) -> Box<dyn Future<Item = Vec<Account>, Error = ()> + Send> {
        let decryption_key = self.decryption_key.clone();
        Box::new(
            cmd("SMEMBERS")
                .arg("send_routes_to")
                .query_async(self.connection.clone())
                .map_err(|err| error!("Error getting members of set send_routes_to: {:?}", err))
                .and_then(
                    move |(connection, account_ids): (RedisReconnect, Vec<AccountId>)| {
                        if account_ids.is_empty() {
                            Either::A(ok(Vec::new()))
                        } else {
                            let mut script = LOAD_ACCOUNTS.prepare_invoke();
                            for id in account_ids.iter() {
                                if !ignore_accounts.contains(id) {
                                    script.arg(id.to_string());
                                }
                            }
                            Either::B(
                                script
                                    .invoke_async(connection.get_shared_connection())
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
                                                    account.decrypt_tokens(
                                                        &decryption_key.expose_secret().0,
                                                    )
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
                .query_async(self.connection.clone())
                .map_err(|err| {
                    error!(
                        "Error getting members of set receive_routes_from: {:?}",
                        err
                    )
                })
                .and_then(
                    |(connection, account_ids): (RedisReconnect, Vec<AccountId>)| {
                        if account_ids.is_empty() {
                            Either::A(ok(Vec::new()))
                        } else {
                            let mut script = LOAD_ACCOUNTS.prepare_invoke();
                            for id in account_ids.iter() {
                                script.arg(id.to_string());
                            }
                            Either::B(
                                script
                                    .invoke_async(connection.get_shared_connection())
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
                                                    account.decrypt_tokens(
                                                        &decryption_key.expose_secret().0,
                                                    )
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
            .query_async(self.connection.clone())
            .map_err(|err| error!("Error getting static routes: {:?}", err))
            .and_then(
                |(_, static_routes): (RedisReconnect, Vec<(String, AccountId)>)| Ok(static_routes),
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
            pipe.query_async(self.connection.clone())
                .map_err(|err| error!("Error setting routes: {:?}", err))
                .and_then(move |(connection, _): (RedisReconnect, Value)| {
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
                pipe.query_async(self.connection.clone())
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
                    .query_async(self.connection.clone())
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
                .query_async(self.connection.clone())
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
            pipe.query_async(self.connection.clone())
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
            .invoke_async(self.connection.get_shared_connection())
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
                .invoke_async(self.connection.get_shared_connection())
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

#[derive(Debug, Clone)]
struct AmountWithScale {
    num: BigUint,
    scale: u8,
}

impl ToRedisArgs for AmountWithScale {
    fn write_redis_args<W>(&self, out: &mut W)
    where
        W: ?Sized + RedisWrite,
    {
        let mut rv = Vec::new();
        self.num.to_string().write_redis_args(&mut rv);
        self.scale.to_string().write_redis_args(&mut rv);
        ToRedisArgs::make_arg_vec(&rv, out);
    }
}

impl AmountWithScale {
    fn parse_multi_values(items: &[Value]) -> Option<Self> {
        // We have to iterate over all values because in this case we're making
        // an lrange call. This returns all the tuple elements in 1 array, and
        // it cannot differentiate between 1 AmountWithScale value or multiple
        // ones. This looks like a limitation of redis.rs
        let len = items.len();
        let mut iter = items.iter();

        let mut max_scale = 0;
        let mut amounts = Vec::new();
        // if redis.rs could parse this properly, we could remove this loop,
        // take 2 elements from the items iterator and return. Then we'd perform
        // the summation and scaling in the consumer of the returned vector.
        for _ in (0..len).step_by(2) {
            let num: String = match iter.next().map(FromRedisValue::from_redis_value) {
                Some(Ok(n)) => n,
                _ => return None,
            };
            let num = match BigUint::from_str(&num) {
                Ok(a) => a,
                Err(_) => return None,
            };

            let scale: u8 = match iter.next().map(FromRedisValue::from_redis_value) {
                Some(Ok(c)) => c,
                _ => return None,
            };

            if scale > max_scale {
                max_scale = scale;
            }
            amounts.push((num, scale));
        }

        // We must scale them to the largest scale, and then add them together
        let mut sum = BigUint::from(0u32);
        for amount in &amounts {
            sum += amount
                .0
                .normalize_scale(ConvertDetails {
                    from: amount.1,
                    to: max_scale,
                })
                .unwrap();
        }

        Some(AmountWithScale {
            num: sum,
            scale: max_scale,
        })
    }
}

impl FromRedisValue for AmountWithScale {
    fn from_redis_value(v: &Value) -> Result<Self, RedisError> {
        if let Value::Bulk(ref items) = *v {
            if let Some(result) = Self::parse_multi_values(items) {
                return Ok(result);
            }
        }
        Err(RedisError::from((
            ErrorKind::TypeError,
            "Cannot parse amount with scale",
        )))
    }
}

impl LeftoversStore for RedisStore {
    type AccountId = AccountId;
    type AssetType = BigUint;

    fn get_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
    ) -> Box<dyn Future<Item = (Self::AssetType, u8), Error = ()> + Send> {
        let mut pipe = redis::pipe();
        pipe.atomic();
        // get the amounts and instantly delete them
        pipe.lrange(uncredited_amount_key(account_id.to_string()), 0, -1);
        pipe.del(uncredited_amount_key(account_id.to_string()))
            .ignore();
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| error!("Error getting uncredited_settlement_amount {:?}", err))
                .and_then(move |(_, amounts): (_, Vec<AmountWithScale>)| {
                    // this call will only return 1 element
                    let amount = amounts[0].clone();
                    Ok((amount.num, amount.scale))
                }),
        )
    }

    fn save_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        uncredited_settlement_amount: (Self::AssetType, u8),
    ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        trace!(
            "Saving uncredited_settlement_amount {:?} {:?}",
            account_id,
            uncredited_settlement_amount
        );
        Box::new(
            // We store these amounts as lists of strings
            // because we cannot do BigNumber arithmetic in the store
            // When loading the amounts, we convert them to the appropriate data
            // type and sum them up.
            cmd("RPUSH")
                .arg(uncredited_amount_key(account_id))
                .arg(AmountWithScale {
                    num: uncredited_settlement_amount.0,
                    scale: uncredited_settlement_amount.1,
                })
                .query_async(self.connection.clone())
                .map_err(move |err| error!("Error saving uncredited_settlement_amount: {:?}", err))
                .and_then(move |(_conn, _ret): (_, Value)| Ok(())),
        )
    }

    fn load_uncredited_settlement_amount(
        &self,
        account_id: Self::AccountId,
        local_scale: u8,
    ) -> Box<dyn Future<Item = Self::AssetType, Error = ()> + Send> {
        let connection = self.connection.clone();
        trace!("Loading uncredited_settlement_amount {:?}", account_id);
        Box::new(
            self.get_uncredited_settlement_amount(account_id)
                .and_then(move |amount| {
                    // scale the amount from the max scale to the local scale, and then
                    // save any potential leftovers to the store
                    let (scaled_amount, precision_loss) =
                        scale_with_precision_loss(amount.0, local_scale, amount.1);
                    if precision_loss > BigUint::from(0u32) {
                        Either::A(
                            cmd("RPUSH")
                                .arg(uncredited_amount_key(account_id))
                                .arg(AmountWithScale {
                                    num: precision_loss,
                                    scale: std::cmp::max(local_scale, amount.1),
                                })
                                .query_async(connection.clone())
                                .map_err(move |err| {
                                    error!("Error saving uncredited_settlement_amount: {:?}", err)
                                })
                                .and_then(move |(_conn, _ret): (_, Value)| Ok(scaled_amount)),
                        )
                    } else {
                        Either::B(ok(scaled_amount))
                    }
                }),
        )
    }
}

type RouteVec = Vec<(String, AccountId)>;

// TODO replace this with pubsub when async pubsub is added upstream: https://github.com/mitsuhiko/redis-rs/issues/183
fn update_routes(
    connection: RedisReconnect,
    routing_table: Arc<RwLock<HashMap<Bytes, AccountId>>>,
) -> impl Future<Item = (), Error = ()> {
    let mut pipe = redis::pipe();
    pipe.hgetall(ROUTES_KEY)
        .hgetall(STATIC_ROUTES_KEY)
        .get(DEFAULT_ROUTE_KEY);
    pipe.query_async(connection)
        .map_err(|err| error!("Error polling for routing table updates: {:?}", err))
        .and_then(
            move |(_connection, (routes, static_routes, default_route)): (
                _,
                (RouteVec, RouteVec, Option<AccountId>),
            )| {
                trace!(
                    "Loaded routes from redis. Static routes: {:?}, default route: {:?}, other routes: {:?}",
                    static_routes,
                    default_route,
                    routes
                );
                // If there is a default route set in the db,
                // set the entry for "" in the routing table to route to that account
                let default_route_iter = iter::once(default_route)
                    .filter_map(|r| r)
                    .map(|account_id| (String::new(), account_id));
                let routes = HashMap::from_iter(
                    routes
                        .into_iter()
                        // Include the default route if there is one
                        .chain(default_route_iter)
                        // Having the static_routes inserted after ensures that they will overwrite
                        // any routes with the same prefix from the first set
                        .chain(static_routes.into_iter())
                        .map(|(prefix, account_id)| (Bytes::from(prefix), account_id)),
                );
                // TODO we may not want to print this because the routing table will be very big
                // if the node has a lot of local accounts
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
