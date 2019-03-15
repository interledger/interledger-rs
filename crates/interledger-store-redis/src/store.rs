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
use redis::{self, cmd, r#async::SharedConnection, Client, PipelineCommands};
use std::{
    iter::FromIterator,
    sync::Arc,
    time::{Duration, Instant},
};
use stream_cancel::{Trigger, Valve};
use tokio_executor::spawn;
use tokio_timer::Interval;

const POLL_INTERVAL: u64 = 60; // 1 minute

static ACCOUNT_FROM_BTP_AUTH_SCRIPT: &str = "return redis.call('HGETALL', 'accounts:' .. redis.call('HGET', 'btp_auth', ARGV[1]) .. ':details')";
static ACCOUNT_FROM_HTTP_AUTH_SCRIPT: &str = "return redis.call('HGETALL', 'accounts:' .. redis.call('HGET', 'http_auth', ARGV[1]) .. ':details')";

fn account_details_key(account_id: u64) -> String {
    format!("accounts:{}:details", account_id)
}

fn account_balance_key(account_id: u64) -> String {
    format!("accounts:{}:balance", account_id)
}

pub fn connect(redis_uri: &str) -> impl Future<Item = RedisStore, Error = ()> {
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
                connection,
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
    connection: SharedConnection,
    exchange_rates: Arc<RwLock<HashMap<String, f64>>>,
    routes: Arc<RwLock<HashMap<Bytes, u64>>>,
    close_connection: Arc<Mutex<Option<Trigger>>>,
}

impl RedisStore {
    pub fn insert_account(&self, account: AccountDetails) -> impl Future<Item = (), Error = ()> {
        let connection = self.connection.clone();
        let mut pipe = redis::pipe();
        pipe.atomic()
            .set(account_balance_key(account.id), 0u64)
            .ignore();
        // TODO hard fail if the auth already exists
        if let Some(ref auth) = account.btp_incoming_authorization {
            pipe.cmd("HSETNX")
                .arg("btp_auth")
                .arg(auth.clone().to_string())
                .arg(account.id);
        }
        if let Some(ref auth) = account.http_incoming_authorization {
            pipe.cmd("HSETNX")
                .arg("http_auth")
                .arg(auth.clone().to_string())
                .arg(account.id);
        }
        pipe.cmd("HMSET")
            .arg(account_details_key(account.id))
            .arg(account)
            .ignore();

        pipe.query_async(connection)
            .and_then(|(_connection, _): (_, u64)| Ok(()))
            .map_err(|err| error!("Error inserting account into DB: {:?}", err))
    }

    // TODO replace this with pubsub when async pubsub is added upstream: https://github.com/mitsuhiko/redis-rs/issues/183
    fn update_rates(&self) -> impl Future<Item = (), Error = ()> {
        let exchange_rates = self.exchange_rates.clone();
        cmd("HGETALL")
            .arg("rates")
            .query_async(self.connection.clone())
            .map_err(|err| error!("Error polling for exchange rates: {:?}", err))
            .and_then(move |(_connection, rates): (_, Vec<(String, f64)>)| {
                let num_assets = rates.len();
                let rates = HashMap::from_iter(rates.into_iter());
                (*exchange_rates.write()) = rates;
                debug!("Updated rates for {} assets", num_assets);
                Ok(())
            })
    }

    fn update_routes(&self) -> impl Future<Item = (), Error = ()> {
        let routing_table = self.routes.clone();
        cmd("HGETALL")
            .arg("routes")
            .query_async(self.connection.clone())
            .map_err(|err| error!("Error polling for routing table updates: {:?}", err))
            .and_then(move |(_connection, routes): (_, Vec<(Vec<u8>, u64)>)| {
                let num_routes = routes.len();
                let routes = HashMap::from_iter(
                    routes
                        .into_iter()
                        .map(|(prefix, account_id)| (Bytes::from(prefix), account_id)),
                );
                (*routing_table.write()) = routes;
                debug!("Updated routing table with {} routes", num_routes);
                Ok(())
            })
    }
}

impl Drop for RedisStore {
    fn drop(&mut self) {
        // two for each of the pollers
        if Arc::strong_count(&self.close_connection) == 3 {
            drop(self.close_connection.lock().take())
        }
    }
}

impl AccountStore for RedisStore {
    type Account = Account;

    fn get_accounts(
        &self,
        account_ids: Vec<<Self::Account as AccountTrait>::AccountId>,
    ) -> Box<Future<Item = Vec<Account>, Error = ()> + Send> {
        let mut pipe = redis::pipe();
        for account_id in account_ids.iter() {
            pipe.cmd("HGETALL").arg(account_details_key(*account_id));
        }
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| {
                    error!(
                        "Error querying details for accounts: {:?} {:?}",
                        account_ids, err
                    )
                })
                .and_then(|(_conn, account): (_, Vec<Account>)| Ok(account)),
        )
    }
}

impl BalanceStore for RedisStore {
    fn get_balance(&self, account: &Self::Account) -> Box<Future<Item = u64, Error = ()> + Send> {
        let account_id = account.id();
        let mut pipe = redis::pipe();
        pipe.get(account_balance_key(account_id));
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| {
                    error!(
                        "Error getting balance for account: {} {:?}",
                        account_id, err
                    )
                })
                .and_then(|(_connection, balance): (_, u64)| Ok(balance)),
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

        // TODO check against balance limit
        let mut pipe = redis::pipe();
        pipe.atomic()
            .cmd("INCRBY")
            .arg(account_balance_key(from_account_id))
            .arg(incoming_amount)
            .cmd("DECRBY")
            .arg(account_balance_key(to_account_id))
            .arg(outgoing_amount);

        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(move |err| {
                    error!(
                    "Error updating balances for accounts. from_account: {}, to_account: {} {:?}",
                    from_account_id,
                    to_account_id,
                    err
                )
                })
                .and_then(move |(_connection, balances): (_, Vec<u64>)| {
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
        let mut pipe = redis::pipe();
        pipe.cmd("EVAL")
            .arg(ACCOUNT_FROM_BTP_AUTH_SCRIPT)
            .arg(token);
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(|err| error!("Error getting account from BTP token: {:?}", err))
                .and_then(|(_connection, account): (_, Account)| Ok(account)),
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
            .arg(auth_header);
        Box::new(
            pipe.query_async(self.connection.clone())
                .map_err(|err| error!("Error getting account from BTP token: {:?}", err))
                .and_then(|(_connection, account): (_, Account)| Ok(account)),
        )
    }
}

impl RouterStore for RedisStore {
    fn routing_table(&self) -> HashMap<Bytes, u64> {
        (*self.routes.read()).clone()
    }
}
