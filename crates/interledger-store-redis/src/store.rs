use super::account::*;
use bytes::Bytes;
use futures::{future::result, Future, Stream};
use hashbrown::HashMap;
use interledger_btp::BtpStore;
use interledger_http::HttpStore;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, AccountStore};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use parking_lot::{Mutex, RwLock};
use redis::{
    self, cmd, r#async::SharedConnection, Client, FromRedisValue, PipelineCommands, Value,
};
use std::{
    iter::FromIterator,
    sync::Arc,
    time::{Duration, Instant},
};
use stream_cancel::{Trigger, Valve};
use tokio_executor::spawn;
use tokio_timer::Interval;

const POLL_INTERVAL: u64 = 60; // 1 minute

static ACCOUNT_FROM_BTP_AUTH_SCRIPT: &str =
    "return redis.call('HGETALL', 'accounts:' .. redis.call('HGET', 'btp_auth', ARGV[1]))";
static ACCOUNT_FROM_HTTP_AUTH_SCRIPT: &str =
    "return redis.call('HGETALL', 'accounts:' .. redis.call('HGET', 'http_auth', ARGV[1]))";
static ROUTES_KEY: &str = "routes";
static RATES_KEY: &str = "rates";
static BALANCES_KEY: &str = "balances";

fn account_details_key(account_id: u64) -> String {
    format!("accounts:{}", account_id)
}

pub fn connect(redis_uri: &str) -> impl Future<Item = RedisStore, Error = ()> {
    debug!("Connecting to Redis: {}", redis_uri);
    result(Client::open(redis_uri))
        .map_err(|err| error!("Error creating Redis client: {:?}", err))
        .and_then(|client| {
            client
                .get_shared_async_connection()
                .map_err(|err| error!("Error connecting to Redis: {:?}", err))
        })
        .and_then(|connection| {
            let (trigger, valve) = Valve::new();
            let store = RedisStore {
                connection: Some(connection),
                exchange_rates: Arc::new(RwLock::new(HashMap::new())),
                routes: Arc::new(RwLock::new(HashMap::new())),
                close_connection: Arc::new(Mutex::new(Some(trigger))),
            };

            // Start polling for rate updates
            // Note: if this behavior changes, make sure to update the Drop implementation
            let store_clone = store.clone();
            let poll_rates = valve
                .wrap(Interval::new(
                    Instant::now(),
                    Duration::from_secs(POLL_INTERVAL),
                ))
                .map_err(|err| error!("Interval error: {:?}", err))
                .for_each(move |_| store_clone.update_rates());
            spawn(poll_rates);

            // Poll for routing table updates
            // Note: if this behavior changes, make sure to update the Drop implementation
            let store_clone = store.clone();
            let poll_routes = valve
                .wrap(Interval::new(
                    Instant::now(),
                    Duration::from_secs(POLL_INTERVAL),
                ))
                .map_err(|err| error!("Interval error: {:?}", err))
                .for_each(move |_| store_clone.update_routes());
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
    // This is only an option for testing purposes
    connection: Option<SharedConnection>,
    exchange_rates: Arc<RwLock<HashMap<String, f64>>>,
    routes: Arc<RwLock<HashMap<Bytes, u64>>>,
    close_connection: Arc<Mutex<Option<Trigger>>>,
}

impl RedisStore {
    pub fn insert_account(&self, account: AccountDetails) -> impl Future<Item = (), Error = ()> {
        debug!("Inserting account: {:?}", account);
        let connection = self.connection.clone();
        let mut pipe = redis::pipe();
        pipe.atomic().hset(BALANCES_KEY, account.id, 0u64).ignore();
        // TODO hard fail if the auth already exists
        if let Some(ref auth) = account.btp_incoming_authorization {
            pipe.cmd("HSETNX")
                .arg("btp_auth")
                .arg(auth.clone().to_string())
                .arg(account.id)
                .ignore();
        }
        if let Some(ref auth) = account.http_incoming_authorization {
            pipe.cmd("HSETNX")
                .arg("http_auth")
                .arg(auth.clone().to_string())
                .arg(account.id)
                .ignore();
        }
        pipe.hset(ROUTES_KEY, account.ilp_address.to_vec(), account.id)
            .ignore();
        pipe.cmd("HMSET")
            .arg(account_details_key(account.id))
            .arg(account)
            .ignore();

        let store = self.clone();
        pipe.query_async(connection.unwrap())
            .and_then(|(_connection, _ret): (_, Value)| Ok(()))
            .map_err(|err| error!("Error inserting account into DB: {:?}", err))
            .and_then(move |_| store.update_routes())
    }

    // TODO replace this with pubsub when async pubsub is added upstream: https://github.com/mitsuhiko/redis-rs/issues/183
    fn update_rates(&self) -> impl Future<Item = (), Error = ()> {
        let exchange_rates = self.exchange_rates.clone();
        cmd("HGETALL")
            .arg(RATES_KEY)
            .query_async(self.connection.clone().unwrap())
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
    fn update_routes(&self) -> impl Future<Item = (), Error = ()> {
        let routing_table = self.routes.clone();
        cmd("HGETALL")
            .arg(ROUTES_KEY)
            .query_async(self.connection.clone().unwrap())
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
}

impl Drop for RedisStore {
    fn drop(&mut self) {
        // one for each of the pollers
        if Arc::strong_count(&self.close_connection) == 3 {
            debug!("Closing connection to Redis");
            drop(self.close_connection.lock().take())
        }
    }
}

impl AccountStore for RedisStore {
    type Account = Account;

    // TODO cache results to avoid hitting Redis for each packet
    fn get_accounts(
        &self,
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
    ) -> Box<Future<Item = Vec<Account>, Error = ()> + Send> {
        let mut pipe = redis::pipe();
        for account_id in account_ids.iter() {
            pipe.cmd("HGETALL").arg(account_details_key(*account_id));
        }
        Box::new(
            pipe.query_async(self.connection.clone().unwrap())
                .map_err(move |err| {
                    error!(
                        "Error querying details for accounts: {:?} {:?}",
                        account_ids, err
                    )
                })
                .and_then(|(_conn, accounts): (_, Vec<Account>)| Ok(accounts)),
        )
    }
}

impl BalanceStore for RedisStore {
    fn get_balance(&self, account: &Self::Account) -> Box<Future<Item = i64, Error = ()> + Send> {
        let account_id = account.id();
        let mut pipe = redis::pipe();
        pipe.hget(BALANCES_KEY, account_id);
        Box::new(
            pipe.query_async(self.connection.clone().unwrap())
                .map_err(move |err| {
                    error!(
                        "Error getting balance for account: {} {:?}",
                        account_id, err
                    )
                })
                .and_then(|(_connection, balance): (_, i64)| Ok(balance)),
        )
    }

    fn update_balances(
        &self,
        from_account: &Account,
        incoming_amount: u64,
        to_account: &Account,
        outgoing_amount: u64,
    ) -> Box<Future<Item = (), Error = ()> + Send> {
        let from_account_id = from_account.id();
        let to_account_id = to_account.id();

        debug!(
            "Increasing balance of account {} by: {}. Decreasing balance of {} by {}",
            from_account_id, incoming_amount, to_account_id, outgoing_amount
        );

        // TODO check against balance limit
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("HINCRBY")
            .arg(BALANCES_KEY)
            .arg(from_account_id)
            .arg(incoming_amount)
            .cmd("HINCRBY")
            .arg(BALANCES_KEY)
            .arg(to_account_id)
            // TODO make sure this doesn't overflow
            .arg(0i64 - outgoing_amount as i64);

        Box::new(
            pipe.query_async(self.connection.clone().unwrap())
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
        // TODO actually store the username
        _username: Option<&str>,
    ) -> Box<Future<Item = Self::Account, Error = ()> + Send> {
        // TODO make sure it can't do script injection!
        // TODO cache the result so we don't hit redis for every packet (is that necessary if redis is often used as a cache?)
        let mut pipe = redis::pipe();
        pipe.cmd("EVAL")
            .arg(ACCOUNT_FROM_BTP_AUTH_SCRIPT)
            .arg(0)
            .arg(token);
        Box::new(
            pipe.query_async(self.connection.clone().unwrap())
                .map_err(|err| error!("Error getting account from BTP token: {:?}", err))
                .and_then(|(_connection, bulk): (_, Value)| {
                    // TODO why does it return a bulk value?
                    if let Value::Bulk(ref items) = bulk {
                        if !items.is_empty() {
                            Account::from_redis_value(&items[0]).map_err(|err| {
                                error!("Unable to parse Account from response: {:?}", err)
                            })
                        } else {
                            error!("Unable to parse Account from empty response");
                            Err(())
                        }
                    } else {
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
        let mut pipe = redis::pipe();
        pipe.cmd("EVAL")
            .arg(ACCOUNT_FROM_HTTP_AUTH_SCRIPT)
            .arg(0)
            .arg(auth_header);
        Box::new(
            pipe.query_async(self.connection.clone().unwrap())
                .map_err(|err| error!("Error getting account from BTP token: {:?}", err))
                .and_then(|(_connection, account): (_, Account)| Ok(account)),
        )
    }
}

impl RouterStore for RedisStore {
    fn routing_table(&self) -> HashMap<Bytes, u64> {
        self.routes.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_routing_table() {
        let store = RedisStore {
            connection: None,
            exchange_rates: Arc::new(RwLock::new(HashMap::new())),
            routes: Arc::new(RwLock::new(HashMap::new())),
            close_connection: Arc::new(Mutex::new(None)),
        };
        assert!(store.routing_table().is_empty());
    }

    #[test]
    fn gets_routing_table() {
        let routes = Arc::new(RwLock::new(HashMap::new()));
        let store = RedisStore {
            connection: None,
            exchange_rates: Arc::new(RwLock::new(HashMap::new())),
            routes: routes.clone(),
            close_connection: Arc::new(Mutex::new(None)),
        };
        assert!(store.routing_table().is_empty());

        routes.write().insert(Bytes::from("example.destination"), 1);

        assert_eq!(store.routing_table().len(), 1);
        assert_eq!(store.clone().routing_table().len(), 1);
    }

    #[test]
    fn replacing_routing_table() {
        let routes = Arc::new(RwLock::new(HashMap::new()));
        let store = RedisStore {
            connection: None,
            exchange_rates: Arc::new(RwLock::new(HashMap::new())),
            routes: routes.clone(),
            close_connection: Arc::new(Mutex::new(None)),
        };
        assert!(store.routing_table().is_empty());

        *routes.write() =
            HashMap::from_iter(vec![(Bytes::from("example.destination"), 1)].into_iter());

        assert_eq!(store.routing_table().len(), 1);
        assert_eq!(store.clone().routing_table().len(), 1);
    }
}
