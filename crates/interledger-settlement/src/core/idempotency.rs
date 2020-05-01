use super::types::{ApiResponse, ApiResult};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Future;
use futures::TryFutureExt;
use http::StatusCode;
use interledger_errors::IdempotentStoreError;
use interledger_errors::*;
use tracing::error;

/// Data stored for the idempotency features
#[derive(Debug, Clone, PartialEq)]
pub struct IdempotentData {
    /// The HTTP Status Code of the API's response
    pub status: StatusCode,
    /// The body of the API's response
    pub body: Bytes,
    /// The hash of the serialized input parameters which generated the API's response
    pub input_hash: [u8; 32],
}

impl IdempotentData {
    /// Simple constructor
    pub fn new(status: StatusCode, body: Bytes, input_hash: [u8; 32]) -> Self {
        Self {
            status,
            body,
            input_hash,
        }
    }
}

/// Store trait which should be implemented for idempotency related features
#[async_trait]
pub trait IdempotentStore {
    /// Returns the API response that was saved when the idempotency key was used
    /// Also returns a hash of the input data which resulted in the response
    async fn load_idempotent_data(
        &self,
        idempotency_key: String,
    ) -> Result<Option<IdempotentData>, IdempotentStoreError>;

    /// Saves the data that was passed along with the api request for later
    /// The store also saves the hash of the input, so that it errors out on requests
    /// with conflicting input hashes for the same idempotency key
    async fn save_idempotent_data(
        &self,
        idempotency_key: String,
        input_hash: [u8; 32],
        status_code: StatusCode,
        data: Bytes,
    ) -> Result<(), IdempotentStoreError>;
}

/// Helper function that returns any idempotent data that corresponds to a
/// provided idempotency key. It fails if the hash of the input that
/// generated the idempotent data does not match the hash of the provided input.
async fn check_idempotency<S>(
    store: S,
    idempotency_key: String,
    input_hash: [u8; 32],
) -> Result<Option<(StatusCode, Bytes)>, ApiError>
where
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    let ret: Option<IdempotentData> = store
        .load_idempotent_data(idempotency_key.clone())
        .map_err(move |_| IDEMPOTENT_STORE_CALL_ERROR.clone())
        .await?;

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
}

// make_idempotent_call takes a function instead of direct arguments so that we
// can reuse it for both the messages and the settlements calls
pub async fn make_idempotent_call<S>(
    store: S,
    non_idempotent_function: impl Future<Output = ApiResult>,
    input_hash: [u8; 32],
    idempotency_key: Option<String>,
    // As per the spec, the success status code is independent of the
    // implemented engine's functionality
    status_code: StatusCode,
    // The default value is used when the engine returns a default return type
    default_return_value: Bytes,
) -> Result<(StatusCode, Bytes), ApiError>
where
    S: IdempotentStore + Clone + Send + Sync + 'static,
{
    if let Some(idempotency_key) = idempotency_key {
        // If there an idempotency key was provided, check idempotency
        match check_idempotency(store.clone(), idempotency_key.clone(), input_hash).await? {
            Some(ret) => {
                if ret.0.is_success() {
                    // Return an OK response if the idempotent call was successful
                    Ok((ret.0, ret.1))
                } else {
                    // Return an HTTP Error otherwise
                    let err_msg = ApiErrorType {
                        r#type: &ProblemType::Default,
                        status: ret.0,
                        title: "Idempotency Error",
                    };
                    // if check_idempotency returns an error, then it
                    // has to be an idempotency error
                    let ret_error = ApiError::from_api_error_type(&err_msg)
                        .detail(String::from_utf8_lossy(&ret.1).to_string());
                    Err(ret_error)
                }
            }
            None => {
                // If there was no previous entry, make the idempotent call and save it
                // Note: The error is also saved idempotently
                let ret = match non_idempotent_function.await {
                    Ok(r) => r,
                    Err(ret) => {
                        let status_code = ret.status;
                        let data = Bytes::from(ret.detail.clone().unwrap_or_default());
                        if store
                            .save_idempotent_data(idempotency_key, input_hash, status_code, data)
                            .await
                            .is_err()
                        {
                            // Should we be panicking here instead?
                            error!("Failed to connect to the store! The request will not be idempotent if retried.")
                        }
                        return Err(ret);
                    }
                };

                let data = match ret {
                    ApiResponse::Default => default_return_value,
                    ApiResponse::Data(d) => d,
                };
                // TODO refactor for readability, can unify the 2 idempotency calls, the error and the data
                // are both Bytes
                store
                    .save_idempotent_data(
                        idempotency_key,
                        input_hash,
                        status_code,
                        data.clone(),
                    )
                    .map_err(move |_| {
                        error!("Failed to connect to the store! The request will not be idempotent if retried.");
                            IDEMPOTENT_STORE_CALL_ERROR.clone()
                    }).await?;

                Ok((status_code, data))
            }
        }
    } else {
        // otherwise just make the call w/o any idempotency saves
        let data = match non_idempotent_function.await? {
            ApiResponse::Default => default_return_value,
            ApiResponse::Data(d) => d,
        };
        Ok((status_code, data))
    }
}
