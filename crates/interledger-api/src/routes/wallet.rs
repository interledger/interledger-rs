use futures::{
    future::{err, ok, Either},
    Future,
};
use hyper::{Response, StatusCode};
use interledger_http::{error::*, HttpAccount, HttpStore};
use interledger_service::{Account, AccountStore, AuthToken, Username};
use interledger_settlement::{
    api::SettlementClient,
    core::{
        get_hash_of,
        idempotency::*,
        types::{ApiResponse, SettlementAccount, SettlementStore, NO_ENGINE_CONFIGURED_ERROR_TYPE},
    },
};
use log::error;
use serde::Serialize;
use std::str::FromStr;
use warp::{self, Filter, Rejection};

pub fn wallet_api<S, A>(
    admin_api_token: String,
    store: S,
) -> warp::filters::BoxedFilter<(impl warp::Reply,)>
where
    S: AccountStore<Account = A>
        + SettlementStore<Account = A>
        + HttpStore<Account = A>
        + IdempotentStore
        + Clone
        + Send
        + Sync
        + 'static,
    A: SettlementAccount + HttpAccount + Account + Serialize + Send + Sync + Clone + 'static,
{
    // Helper filters
    let idempotency = warp::header::optional::<String>("idempotency-key");
    let accounts = warp::path("accounts");
    let with_store = warp::any().map(move || store.clone()).boxed();
    let admin_auth_header = format!("Bearer {}", admin_api_token);
    let with_admin_auth_header = warp::any().map(move || admin_auth_header.clone()).boxed();
    // Receives parameters which were prepared by `account_username` and
    // considers the request is eligible to be processed or not, checking the auth.
    // Why we separate `account_username` and this filter is that
    // we want to check whether the sender is eligible to access this path but at the same time,
    // we don't want to spawn any `Rejection`s at `account_username`.
    // At the point of `account_username`, there might be some other
    // remaining path filters. So we have to process those first, not to spawn errors of
    // unauthorized that the the request actually should not cause.
    // This function needs parameters which can be prepared by `account_username`.
    let account_username = accounts
        .and(warp::path::param2::<Username>())
        .and_then(|username: Username| -> Result<_, Rejection> {
            warp::filters::ext::set(username);
            Ok(())
        })
        .untuple_one()
        .boxed();
    let admin_or_authorized_user_only = warp::filters::ext::get::<Username>()
        .and(warp::header::<String>("authorization"))
        .and(with_store.clone())
        .and(with_admin_auth_header.clone())
        .and_then(
            |path_username: Username, auth_string: String, store: S, admin_auth_header: String| {
                store.get_account_id_from_username(&path_username).then(
                    move |account_id: Result<A::AccountId, _>| {
                        if account_id.is_err() {
                            return Either::A(err::<A::AccountId, Rejection>(
                                ApiError::account_not_found().into(),
                            ));
                        }
                        let account_id = account_id.unwrap();
                        if auth_string == admin_auth_header {
                            return Either::A(ok(account_id));
                        }
                        let auth = match AuthToken::from_str(&auth_string) {
                            Ok(auth) => auth,
                            Err(_) => return Either::A(err(ApiError::account_not_found().into())),
                        };
                        Either::B(
                            store
                                .get_account_from_http_auth(auth.username(), auth.password())
                                .then(move |authorized_account: Result<A, _>| {
                                    if authorized_account.is_err() {
                                        return err(ApiError::unauthorized().into());
                                    }
                                    let authorized_account = authorized_account.unwrap();
                                    if &path_username == authorized_account.username() {
                                        ok(authorized_account.id())
                                    } else {
                                        err(ApiError::unauthorized().into())
                                    }
                                }),
                        )
                    },
                )
            },
        )
        .boxed();

    // GET /accounts/:username/deposit
    // This will return a json of the address that the client needs to send money to
    let deposit_endpoint = account_username.clone().and(warp::path("deposit"));
    let deposit = warp::get2()
        .and(deposit_endpoint)
        .and(warp::path::end())
        .and(admin_or_authorized_user_only.clone())
        .and(with_store.clone())
        .and_then(move |account_id: A::AccountId, store: S| {
            let client = SettlementClient::new();
            // Convert to the desired data types
            store
                .get_accounts(vec![account_id])
                .map_err(move |_err| {
                    let err = ApiError::account_not_found()
                        .detail(format!("Account {} was not found", account_id));
                    error!("{}", err);
                    err.into()
                })
                .and_then(move |accounts| {
                    let account = &accounts[0];
                    client
                        .get_payment_info(account.clone())
                        .map_err::<_, Rejection>(move |_| ApiError::internal_server_error().into())
                        .and_then(move |message| {
                            Ok(Response::builder()
                                .status(StatusCode::OK)
                                .body(message.to_string())
                                .unwrap())
                        })
                })
        });

    // POST /accounts/:username/withdrawals (optional idempotency-key header)
    // This will withdraw funds from the node. e.g if you have 50 units, you are allowed to execute a 50 unit withdrawal via the engine: the body should contain the withdrawal information as a bytes array, whcih the engine will interpret
    let withdrawals_endpoint = account_username.and(warp::path("withdrawals"));
    let withdrawals = warp::post2()
        .and(withdrawals_endpoint)
        .and(warp::path::end())
        .and(admin_or_authorized_user_only)
        .and(idempotency)
        .and(warp::body::json())
        .and(with_store.clone())
        .and_then(
            move |account_id: A::AccountId,
                  idempotency_key: Option<String>,
                  // This has to be u64 sinc we're going to try subtracting it from our balance in the connector
                  quantity: u64,
                  store: S| {
                let input = format!("{}{:?}", account_id, quantity);
                let input_hash = get_hash_of(input.as_ref());

                let store_clone = store.clone();
                let withdraw_fn = move || do_withdraw(store_clone, account_id, quantity);
                make_idempotent_call(
                    store,
                    withdraw_fn,
                    input_hash,
                    idempotency_key,
                    StatusCode::CREATED,
                    "WITHDREW".into(),
                )
                .map_err::<_, Rejection>(move |err| err.into())
                .and_then(move |(status_code, message)| {
                    Ok(Response::builder()
                        .status(status_code)
                        .body(message)
                        .unwrap())
                })
            },
        );

    deposit.or(withdrawals).boxed()
}

fn do_withdraw<S, A>(
    store: S,
    account_id: A::AccountId,
    // assume this is in the account's scale
    amount: u64,
) -> Box<dyn Future<Item = ApiResponse, Error = ApiError> + Send>
where
    S: SettlementStore<Account = A> + AccountStore<Account = A> + Clone + Send + Sync + 'static,
    A: SettlementAccount + Account + Send + Sync + Clone + 'static,
{
    let store_clone = store.clone();
    // Convert to the desired data types

    // TODO: currently we do fetch account_id -> check if has engine details -> reduce its balance -> if successful send message to engine, we should be able to reduce these db calls, even though they "should" be infrequent
    Box::new(
        store.get_accounts(vec![account_id])
        .map_err(move |_err| {
            let err = ApiError::account_not_found().detail(format!("Account {} was not found", account_id));
            error!("{}", err);
            err
        })
        .and_then(move |accounts| {
            let account = &accounts[0];
            if account.settlement_engine_details().is_some() {
                Ok(account.clone())
            } else {
                let error_msg = format!("Account {} does not have settlement engine details configured. Will not attempt to withdraw", account.id());
                error!("{}", error_msg);
                Err(ApiError::from_api_error_type(&NO_ENGINE_CONFIGURED_ERROR_TYPE).detail(error_msg))
            }
        })
        .and_then(move |account| {
            // TODO: is there any race condition where this future completes and executes the request on the engine simultaneously with another call?
            store_clone.withdraw_funds(account_id, amount)
            .map_err(move |_err| {
                let error_msg = format!("Error reducing account's balance: {}", account_id);
                error!("{}", error_msg);
                let error_type = ApiErrorType {
                    r#type: &ProblemType::Default,
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    title: "Withdraw funds error", 
                };
                ApiError::from_api_error_type(&error_type).detail(error_msg)
            })
            .and_then(move |_| {
                let client = SettlementClient::new();
                client.send_settlement(account.clone(), amount)
                .map_err(move |_err| {
                    let error_msg = format!("Error executing withdrawal from engine for account: {}", account_id);
                    error!("{}", error_msg);
                    let error_type = ApiErrorType {
                        r#type: &ProblemType::Default,
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        title: "Execute withdrawal error", 
                    };
                    ApiError::from_api_error_type(&error_type).detail(error_msg)
                })
                .and_then(move |_| Ok(ApiResponse::Default))
            })
        })
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use interledger_packet::Address;
    use interledger_settlement::core::types::SettlementEngineDetails;
    use lazy_static::lazy_static;
    use mockito::{mock, Matcher};
    use parking_lot::RwLock;
    use std::sync::Arc;
    use url::Url;

    lazy_static! {
        pub static ref ALICE: Username = Username::from_str("alice").unwrap();
        pub static ref ALICE_ADDR: Address = Address::from_str("example.alice").unwrap();
        pub static ref URL: Url = Url::parse("http://localhost:3000").unwrap();
    }

    // Withdrawal Tests
    mod withdrawals_tests {
        use super::*;

        #[allow(dead_code)]
        pub fn mock_settlement(status_code: usize) -> mockito::Mock {
            mock(
                "POST",
                Matcher::Regex(r"^/accounts/\d*/settlements$".to_string()),
            )
            // The settlement API receives json data
            .match_header("Content-Type", "application/json")
            .with_status(status_code)
            .with_body("hi")
        }

        fn withdrawal_call<F>(
            api: &F,
            username: &str,
            amount: u64,
            idempotency_key: Option<String>,
        ) -> Response<Bytes>
        where
            F: warp::Filter + 'static,
            F::Extract: warp::Reply,
        {
            let mut response = warp::test::request()
                .method("POST")
                .header("Authorization", "Bearer token")
                .path(&format!("/accounts/{}/withdrawals", username))
                .body(amount.to_string());

            if let Some(idempotency_key) = idempotency_key {
                response = response.header("Idempotency-Key", idempotency_key);
            }
            response.reply(api)
        }

        #[test]
        fn withdrawal_ok() {
            let m = mock_settlement(200).create();
            let se_url = mockito::server_url();
            let store = test_store(se_url);
            let api = test_api(store.clone());

            // will try to reduce the account's balance by 100
            // should fail (Account 0 matches to Alice)
            store.set_balance(0, 200);
            let response = withdrawal_call(&api, "alice", 100, None);
            assert_eq!(response.status(), StatusCode::CREATED);
            assert_eq!(response.body(), "WITHDREW");
            m.assert();
        }
    }

    // Deposit Tests
    mod deposit_tests {
        use super::*;
        use serde_json::json;

        fn mock_deposit() -> mockito::Mock {
            mock(
                "GET",
                Matcher::Regex(r"^/accounts/\d*/deposit$".to_string()),
            )
            .with_status(200)
            // This can be anything by the engine as long as the wallet can interpret it
            .with_body(json!({"address": "some_address", "currency": "ETH"}).to_string())
        }

        fn deposit_call<F>(api: &F, username: &str) -> Response<Bytes>
        where
            F: warp::Filter + 'static,
            F::Extract: warp::Reply,
        {
            warp::test::request()
                .method("GET")
                .header("Authorization", "Bearer token")
                .path(&format!("/accounts/{}/deposit", username))
                .reply(api)
        }

        #[test]
        fn deposit_ok() {
            let se_url = mockito::server_url();
            let store = test_store(se_url);
            let m = mock_deposit().create();
            let api = test_api(store.clone());

            // the engine responds with the payment details
            let response = deposit_call(&api, "alice");
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.body(),
                &Bytes::from(json!({"address": "some_address", "currency": "ETH"}).to_string())
            );
            m.assert();
        }
    }

    #[derive(Debug, Clone, Serialize)]
    pub struct TestAccount {
        pub id: u64,
        pub balance: i64,
        pub url: Url,
    }

    // helpers
    impl Account for TestAccount {
        type AccountId = u64;

        fn id(&self) -> u64 {
            self.id
        }

        fn username(&self) -> &Username {
            &ALICE
        }

        fn asset_code(&self) -> &str {
            "XYZ"
        }

        // All connector accounts use asset scale = 9.
        fn asset_scale(&self) -> u8 {
            9
        }

        fn ilp_address(&self) -> &Address {
            &ALICE_ADDR
        }
    }
    impl SettlementAccount for TestAccount {
        fn settlement_engine_details(&self) -> Option<SettlementEngineDetails> {
            Some(SettlementEngineDetails {
                url: self.url.clone(),
            })
        }
    }

    impl HttpAccount for TestAccount {
        fn get_http_url(&self) -> Option<&Url> {
            Some(&URL)
        }

        fn get_http_auth_token(&self) -> Option<&str> {
            Some("token")
        }
    }

    #[derive(Clone)]
    pub struct TestStore {
        pub accounts: Arc<RwLock<Vec<TestAccount>>>,
    }

    impl SettlementStore for TestStore {
        type Account = TestAccount;

        fn withdraw_funds(
            &self,
            account_id: u64,
            amount: u64,
        ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            let mut accounts = self.accounts.write();
            for mut a in &mut *accounts {
                if a.id() == account_id {
                    if a.balance > amount as i64 {
                        a.balance -= amount as i64
                    } else {
                        return Box::new(err(()));
                    }
                }
            }
            Box::new(ok(()))
        }

        fn update_balance_for_incoming_settlement(
            &self,
            _account_id: u64,
            _amount: u64,
            _idempotency_key: Option<String>,
        ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            unimplemented!()
        }

        fn refund_settlement(
            &self,
            _account_id: u64,
            _settle_amount: u64,
        ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            unimplemented!()
        }
    }

    impl HttpStore for TestStore {
        type Account = TestAccount;

        fn get_account_from_http_auth(
            &self,
            _username: &Username,
            _token: &str,
        ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send> {
            Box::new(
                self.get_accounts(vec![0])
                    .and_then(move |accs| Ok(accs[0].clone())),
            )
        }
    }

    impl IdempotentStore for TestStore {
        fn load_idempotent_data(
            &self,
            _idempotency_key: String,
        ) -> Box<dyn Future<Item = Option<IdempotentData>, Error = ()> + Send> {
            Box::new(ok(None))
        }

        fn save_idempotent_data(
            &self,
            _idempotency_key: String,
            _input_hash: [u8; 32],
            _status_code: StatusCode,
            _data: Bytes,
        ) -> Box<dyn Future<Item = (), Error = ()> + Send> {
            Box::new(ok(()))
        }
    }

    impl TestStore {
        pub fn new(accs: Vec<TestAccount>) -> Self {
            TestStore {
                accounts: Arc::new(RwLock::new(accs)),
            }
        }

        pub fn set_balance(&self, account_id: u64, balance: u64) {
            let mut accounts = self.accounts.write();
            for mut a in &mut *accounts {
                if a.id() == account_id {
                    a.balance = balance as i64;
                }
            }
        }
    }

    impl AccountStore for TestStore {
        type Account = TestAccount;

        fn get_accounts(
            &self,
            account_ids: Vec<<<Self as AccountStore>::Account as Account>::AccountId>,
        ) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send> {
            let accounts: Vec<TestAccount> = self
                .accounts
                .read()
                .iter()
                .filter_map(|account| {
                    if account_ids.contains(&account.id) {
                        Some(account.clone())
                    } else {
                        None
                    }
                })
                .collect();
            if accounts.len() == account_ids.len() {
                Box::new(ok(accounts))
            } else {
                Box::new(err(()))
            }
        }

        // stub implementation (not used in these tests)
        fn get_account_id_from_username(
            &self,
            _username: &Username,
        ) -> Box<dyn Future<Item = u64, Error = ()> + Send> {
            Box::new(ok(0))
        }
    }

    pub fn test_store(url: String) -> TestStore {
        TestStore::new(vec![TestAccount {
            id: 0,
            balance: 0,
            url: Url::parse(&url).unwrap(),
        }])
    }

    pub fn test_api(my_test_store: TestStore) -> warp::filters::BoxedFilter<(impl warp::Reply,)> {
        wallet_api("token".to_owned(), my_test_store)
    }
}
