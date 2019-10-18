use crate::error::*;
use bytes::Bytes;
use futures::executor::spawn;
use futures::{
    future::{err, ok, Either},
    Future,
};
use http::StatusCode;
use log::error;

#[derive(Debug, Clone, PartialEq)]
pub struct IdempotentData {
    pub status: StatusCode,
    pub body: Bytes,
    pub input_hash: [u8; 32],
}

impl IdempotentData {
    pub fn new(status: StatusCode, body: Bytes, input_hash: [u8; 32]) -> Self {
        Self {
            status,
            body,
            input_hash,
        }
    }
}

pub trait IdempotentStore {
    /// Returns the API response that was saved when the idempotency key was used
    /// Also returns a hash of the input data which resulted in the response
    fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Box<dyn Future<Item = Option<IdempotentData>, Error = ()> + Send>;

    /// Saves the data that was passed along with the api request for later
    /// The store MUST also save a hash of the input, so that it errors out on requests
    fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;
}

// Helper function that returns any idempotent data that corresponds to a
// provided idempotency key. It fails if the hash of the input that
// generated the idempotent data does not match the hash of the provided input.
fn check_idempotency<S>(
    store: S,
    idempotency_key: String,
    input_hash: [u8; 32],
) -> impl Future<Item = Option<(StatusCode, Bytes)>, Error = ApiError>
where
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    store
        .load_idempotent_data(idempotency_key.clone())
        .map_err(move |_| IDEMPOTENT_STORE_CALL_ERROR.clone())
        .and_then(move |ret: Option<IdempotentData>| {
            if let Some(ret) = ret {
                // Check if the hash (ret.2) of the loaded idempotent data matches the hash
                // of the provided input data. If not, we should error out since
                // the caller provided an idempotency key that was used for a
                // different input.
                if ret.input_hash == input_hash {
                    Ok(Some((ret.status, ret.body)))
                } else {
                    Ok(Some((
                        StatusCode::from_u16(409).unwrap(),
                        Bytes::from(IDEMPOTENCY_CONFLICT_ERR),
                    )))
                }
            } else {
                Ok(None)
            }
        })
}

// make_idempotent_call takes a function instead of direct arguments so that we
// can reuse it for both the messages and the settlements calls
pub fn make_idempotent_call<S, F>(
    store: S,
    non_idempotent_function: F,
    input_hash: [u8; 32],
    idempotency_key: Option<String>,
) -> impl Future<Item = (StatusCode, Bytes), Error = ApiError>
where
    F: FnOnce() -> Box<dyn Future<Item = (StatusCode, Bytes), Error = ApiError> + Send>,
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    if let Some(idempotency_key) = idempotency_key {
        // If there an idempotency key was provided, check idempotency
        // and the key was not present or conflicting with an existing
        // key, perform the call and save the idempotent return data
        Either::A(
            check_idempotency(store.clone(), idempotency_key.clone(), input_hash).and_then(
                move |ret: Option<(StatusCode, Bytes)>| {
                    if let Some(ret) = ret {
                        if ret.0.is_success() {
                            Either::A(Either::A(ok((ret.0, ret.1))))
                        } else {
                            let err_msg = ApiErrorType {
                                r#type: &ProblemType::Default,
                                status: ret.0,
                                title: "Idempotency Error",
                            };
                            // if check_idempotency returns an error, then it
                            // has to be an idempotency error
                            let ret_error = ApiError::from_api_error_type(&err_msg)
                                .detail(String::from_utf8_lossy(&ret.1).to_string());
                            Either::A(Either::B(err(ret_error)))
                        }
                    } else {
                        Either::B(
                            non_idempotent_function().map_err({
                                let store = store.clone();
                                let idempotency_key = idempotency_key.clone();
                                move |ret: ApiError| {
                                    let status_code = ret.status;
                                    let data = Bytes::from(ret.detail.clone().unwrap_or_default());
                                    spawn(store.save_idempotent_data(
                                        idempotency_key,
                                        input_hash,
                                        status_code,
                                        data,
                                    ).map_err(move |_| error!("Failed to connect to the store! The request will not be idempotent if retried.")));
                                    ret
                                }})
                            .and_then(
                                move |ret: (StatusCode, Bytes)| {
                                    store
                                        .save_idempotent_data(
                                            idempotency_key,
                                            input_hash,
                                            ret.0,
                                            ret.1.clone(),
                                        )
                                        .map_err(move |_| {
                                            error!("Failed to connect to the store! The request will not be idempotent if retried.");
                                             IDEMPOTENT_STORE_CALL_ERROR.clone()
                                        })
                                        .and_then(move |_| Ok((ret.0, ret.1)))
                                },
                            ),
                        )
                    }
                },
            ),
        )
    } else {
        // otherwise just make the call w/o any idempotency saves
        Either::B(
            non_idempotent_function().and_then(move |ret: (StatusCode, Bytes)| Ok((ret.0, ret.1))),
        )
    }
}
