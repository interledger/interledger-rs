use super::account::*;
use bytes::Bytes;
use futures::{
    future::{err, ok, result, Either},
    Future, Stream,
};
use hashbrown::HashMap;
use interledger_api::{AccountDetails, NodeStore};
use interledger_btp::BtpStore;
use interledger_ccp::RouteManagerStore;
use interledger_http::HttpStore;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use parking_lot::RwLock;
use redis::{self, cmd, r#async::SharedConnection, Client, PipelineCommands, Value};
use std::{
    iter::FromIterator,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_executor::spawn;
use tokio_timer::Interval;

const POLL_INTERVAL: u64 = 60000; // 1 minute

// TODO load this into redis cache
static ACCOUNT_FROM_INDEX: &str = "local id = redis.call('HGET', KEYS[1], ARGV[1])
    if not id then
        return nil
    end
    return redis.call('HGETALL', 'accounts:' .. id)";
static ROUTES_KEY: &str = "routes";
static RATES_KEY: &str = "rates";
static NEXT_ACCOUNT_ID_KEY: &str = "next_account_id";

fn account_details_key(account_id: u64) -> String {
    format!("accounts:{}", account_id)
}

fn balance_key(asset_code: &str) -> String {
    format!("balances:{}", asset_code)
}

pub use redis::IntoConnectionInfo;

pub fn connect<R>(redis_uri: R) -> impl Future<Item = RedisStore, Error = ()>
where
    R: IntoConnectionInfo,
{
    connect_with_poll_interval(redis_uri, POLL_INTERVAL)
}

#[doc(hidden)]
pub fn connect_with_poll_interval<R>(
    redis_uri: R,
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
            let store = RedisStore {
                connection: Arc::new(connection),
                exchange_rates: Arc::new(RwLock::new(HashMap::new())),
                routes: Arc::new(RwLock::new(HashMap::new())),
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
}

impl RedisStore {
    fn get_next_account_id(&self) -> impl Future<Item = u64, Error = ()> {
        cmd("INCR")
            .arg(NEXT_ACCOUNT_ID_KEY)
            .query_async(self.connection.as_ref().clone())
            .map_err(|err| error!("Error incrementing account ID: {:?}", err))
            .and_then(|(_conn, next_account_id): (_, u64)| Ok(next_account_id - 1))
    }
}

impl AccountStore for RedisStore {
    type Account = Account;

    // TODO cache results to avoid hitting Redis for each packet
    fn get_accounts(
        &self,
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
    ) -> Box<Future<Item = Vec<Account>, Error = ()> + Send> {
        let num_accounts = account_ids.len();
        let mut pipe = redis::pipe();
        for account_id in account_ids.iter() {
            pipe.cmd("HGETALL").arg(account_details_key(*account_id));
        }
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error querying details for accounts: {:?} {:?}",
                        account_ids, err
                    )
                })
                .and_then(move |(_conn, accounts): (_, Vec<Account>)| {
                    if accounts.len() == num_accounts {
                        Ok(accounts)
                    } else {
                        Err(())
                    }
                }),
        )
    }
}

impl BalanceStore for RedisStore {
    fn get_balance(&self, account: Account) -> Box<Future<Item = i64, Error = ()> + Send> {
        Box::new(
            cmd("HGET")
                .arg(balance_key(account.asset_code.as_str()))
                .arg(account.id)
                .query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                        "Error getting balance for account: {} {:?}",
                        account.id, err
                    )
                })
                .and_then(|(_connection, balance): (_, i64)| Ok(balance)),
        )
    }

    fn update_balances(
        &self,
        from_account: Account,
        incoming_amount: u64,
        to_account: Account,
        outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        let from_account_id = from_account.id();
        let to_account_id = to_account.id();

        debug!(
            "Increasing balance of account {} by: {}. Decreasing balance of account {} by: {}",
            from_account_id, incoming_amount, to_account_id, outgoing_amount
        );

        // TODO check against balance limit
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("HINCRBY")
            .arg(balance_key(from_account.asset_code.as_str()))
            .arg(from_account_id)
            .arg(incoming_amount)
            .cmd("HINCRBY")
            .arg(balance_key(to_account.asset_code.as_str()))
            .arg(to_account_id)
            // TODO make sure this doesn't overflow
            .arg(0i64 - outgoing_amount as i64);

        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                    "Error updating balances for accounts. from_account: {}, to_account: {}: {:?}",
                    from_account_id,
                    to_account_id,
                    err
                )
                })
                .and_then(move |(_connection, balances): (_, Vec<i64>)| {
                    debug!(
                        "Updated account balances. Account {} has: {}, account {} has: {}",
                        from_account_id, balances[0], to_account_id, balances[1]
                    );
                    Ok(())
                }),
        )
    }

    fn undo_balance_update(
        &self,
        from_account: Account,
        incoming_amount: u64,
        to_account: Account,
        outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        let from_account_id = from_account.id();
        let to_account_id = to_account.id();

        debug!(
            "Decreasing balance of account {} by: {}. Increasing balance of account {} by: {}",
            from_account_id, incoming_amount, to_account_id, outgoing_amount
        );

        // TODO check against balance limit
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("HINCRBY")
            .arg(balance_key(from_account.asset_code.as_str()))
            .arg(from_account_id)
            // TODO make sure this doesn't overflow
            .arg(0i64 - incoming_amount as i64)
            .cmd("HINCRBY")
            .arg(balance_key(to_account.asset_code.as_str()))
            .arg(to_account_id)
            .arg(outgoing_amount);

        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(move |err| {
                    error!(
                    "Error undoing balance update for accounts. from_account: {}, to_account: {}: {:?}",
                    from_account_id,
                    to_account_id,
                    err
                )
                })
                .and_then(move |(_connection, balances): (_, Vec<i64>)| {
                    debug!(
                        "Updated account balances. Account {} has: {}, account {} has: {}",
                        from_account_id, balances[0], to_account_id, balances[1]
                    );
                    Ok(())
                }),
        )
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
        let token = token.to_string();
        Box::new(
            cmd("EVAL")
                .arg(ACCOUNT_FROM_INDEX)
                .arg(1)
                .arg("btp_auth")
                .arg(&token)
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error getting account from BTP token: {:?}", err))
                .and_then(move |(_connection, account): (_, Option<Account>)| {
                    if let Some(account) = account {
                        Ok(account)
                    } else {
                        warn!("No account found with BTP token: {}", token);
                        Err(())
                    }
                }),
        )
    }
}

impl HttpStore for RedisStore {
    type Account = Account;

    fn get_account_from_http_auth(
        &self,
        auth_header: &str,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send> {
        // TODO make sure it can't do script injection!
        let auth_header = auth_header.to_string();
        Box::new(
            cmd("EVAL")
                .arg(ACCOUNT_FROM_INDEX)
                .arg(1)
                .arg("http_auth")
                .arg(&auth_header)
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error getting account from HTTP auth: {:?}", err))
                .and_then(move |(_connection, account): (_, Option<Account>)| {
                    if let Some(account) = account {
                        Ok(account)
                    } else {
                        warn!("No account found with HTTP auth: {}", auth_header);
                        Err(())
                    }
                }),
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
        debug!("Inserting account: {:?}", account);
        let connection = self.connection.clone();
        let routing_table = self.routes.clone();

        Box::new(
            self.get_next_account_id()
                .and_then(|id| {
                    debug!("Next account id is: {}", id);
                    Account::try_from(id, account)
                })
                .and_then(move |account| {
                    // Check that there isn't already an account with values that must be unique
                    let mut keys: Vec<String> = vec!["ID".to_string(), "ID".to_string()];

                    let mut pipe = redis::pipe();
                    pipe.cmd("EXISTS")
                        .arg(account_details_key(account.id))
                        .cmd("HEXISTS")
                        .arg(balance_key(account.asset_code.as_str()))
                        .arg(account.id);

                    if let Some(ref auth) = account.btp_incoming_authorization {
                        keys.push("BTP auth".to_string());
                        pipe.cmd("HEXISTS")
                            .arg("btp_auth")
                            .arg(auth.clone().to_string());
                    }
                    if let Some(ref auth) = account.http_incoming_authorization {
                        keys.push("HTTP auth".to_string());
                        pipe.cmd("HEXISTS")
                            .arg("http_auth")
                            .arg(auth.clone().to_string());
                    }
                    if let Some(ref xrp_address) = account.xrp_address {
                        keys.push("XRP address".to_string());
                        pipe.cmd("HEXISTS").arg("xrp_addresses").arg(xrp_address);
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
                .and_then(|(connection, account)| {
                    let mut pipe = redis::pipe();

                    // Set balance
                    pipe.atomic()
                        .cmd("HSET")
                        .arg(balance_key(account.asset_code.as_str()))
                        .arg(account.id)
                        .arg(0u64)
                        .ignore();

                    // Set incoming auth details
                    if let Some(ref auth) = account.btp_incoming_authorization {
                        pipe.cmd("HSET")
                            .arg("btp_auth")
                            .arg(auth.clone().to_string())
                            .arg(account.id)
                            .ignore();
                    }
                    if let Some(ref auth) = account.http_incoming_authorization {
                        pipe.cmd("HSET")
                            .arg("http_auth")
                            .arg(auth.clone().to_string())
                            .arg(account.id)
                            .ignore();
                    }

                    // Add settlement details
                    if let Some(ref xrp_address) = account.xrp_address {
                        pipe.cmd("HSET")
                            .arg("xrp_addresses")
                            .arg(xrp_address)
                            .arg(account.id)
                            .ignore();
                    }

                    if account.send_routes {
                        pipe.cmd("SADD")
                            .arg("send_routes_to")
                            .arg(account.id)
                            .ignore();
                    }

                    // Add route to routing table
                    pipe.hset(ROUTES_KEY, account.ilp_address.to_vec(), account.id)
                        .ignore();

                    // Set account details
                    pipe.cmd("HMSET")
                        .arg(account_details_key(account.id))
                        .arg(account.clone())
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

    // TODO limit the number of results and page through them
    fn get_all_accounts(&self) -> Box<Future<Item = Vec<Self::Account>, Error = ()> + Send> {
        Box::new(
            cmd("GET")
                .arg(NEXT_ACCOUNT_ID_KEY)
                .query_async(self.connection.as_ref().clone())
                .and_then(|(connection, next_account_id): (SharedConnection, u64)| {
                    let mut pipe = redis::pipe();
                    for i in 0..next_account_id {
                        pipe.cmd("HGETALL").arg(account_details_key(i));
                    }
                    pipe.query_async(connection)
                        .and_then(|(_, accounts): (_, Vec<Self::Account>)| Ok(accounts))
                })
                .map_err(|err| error!("Error getting all accounts: {:?}", err)),
        )
    }

    fn set_rates<R>(&self, rates: R) -> Box<Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, f64)>,
    {
        let rates: Vec<(String, f64)> = rates.into_iter().collect();
        let connection = self.connection.as_ref().clone();
        let exchange_rates = self.exchange_rates.clone();
        Box::new(
            cmd("HMSET")
                .arg(RATES_KEY)
                .arg(rates)
                .query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error setting rates: {:?}", err))
                .and_then(move |(_connection, _): (_, Value)| {
                    update_rates(connection, exchange_rates)
                }),
        )
    }
}

impl RouteManagerStore for RedisStore {
    type Account = Account;

    fn get_accounts_to_send_routes_to(
        &self,
    ) -> Box<Future<Item = Vec<Account>, Error = ()> + Send> {
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
                            pipe.cmd("HGETALL").arg(account_details_key(id));
                        }
                        Either::B(
                            pipe.query_async(connection)
                                .map_err(|err| {
                                    error!("Error getting accounts to send routes to: {:?}", err)
                                })
                                .and_then(
                                    |(_connection, accounts): (SharedConnection, Vec<Account>)| {
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
        Box::new(self.get_all_accounts().and_then(|accounts| {
            let local_table = HashMap::from_iter(
                accounts
                    .into_iter()
                    .map(|account| (account.ilp_address.clone(), account)),
            );
            // TODO add configured routes
            Ok((local_table, HashMap::new()))
        }))
    }

    fn set_routes<R>(&mut self, routes: R) -> Box<Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (Bytes, Account)>,
    {
        let routes = routes.into_iter();
        // Update local routes
        let mut local_routes: HashMap<Bytes, u64> = HashMap::with_capacity(routes.size_hint().0);
        let mut db_routes: Vec<(String, u64)> = Vec::with_capacity(routes.size_hint().0);
        for (prefix, account) in routes {
            local_routes.insert(prefix.clone(), account.id);
            if let Ok(prefix) = String::from_utf8(prefix.to_vec()) {
                db_routes.push((prefix, account.id));
            }
        }
        let num_routes = db_routes.len();

        *self.routes.write() = local_routes;

        // Save routes to Redis
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("DEL")
            .arg(ROUTES_KEY)
            .ignore()
            .cmd("HMSET")
            .arg(ROUTES_KEY)
            .arg(db_routes)
            .ignore();
        Box::new(
            pipe.query_async(self.connection.as_ref().clone())
                .map_err(|err| error!("Error setting routes: {:?}", err))
                .and_then(move |(_connection, _): (SharedConnection, Value)| {
                    trace!("Saved {} routes to Redis", num_routes);
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
            debug!("Updated rates for {} assets", num_assets);
            Ok(())
        })
}

// TODO replace this with pubsub when async pubsub is added upstream: https://github.com/mitsuhiko/redis-rs/issues/183
fn update_routes(
    connection: SharedConnection,
    routing_table: Arc<RwLock<HashMap<Bytes, u64>>>,
) -> impl Future<Item = (), Error = ()> {
    cmd("HGETALL")
        .arg(ROUTES_KEY)
        .query_async(connection)
        .map_err(|err| error!("Error polling for routing table updates: {:?}", err))
        .and_then(move |(_connection, routes): (_, Vec<(Vec<u8>, u64)>)| {
            let num_routes = routes.len();
            let routes = HashMap::from_iter(
                routes
                    .into_iter()
                    .map(|(prefix, account_id)| (Bytes::from(prefix), account_id)),
            );
            *routing_table.write() = routes;
            debug!("Updated routing table with {} routes", num_routes);
            Ok(())
        })
}
