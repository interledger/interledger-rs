use interledger_http::error::*;
use lazy_static::lazy_static;

pub static IDEMPOTENCY_CONFLICT_ERR: &str = "Provided idempotency key is tied to other input";

pub const IDEMPOTENT_STORE_CALL_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Store idempotency error",
    status: StatusCode::CONFLICT,
};

pub const CONVERSION_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Conversion error",
    status: StatusCode::INTERNAL_SERVER_ERROR,
};

pub const NO_ENGINE_CONFIGURED_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    status: StatusCode::NOT_FOUND,
    title: "No settlement engine configured",
};

lazy_static! {
    pub static ref IDEMPOTENT_STORE_CALL_ERROR: ApiError =
        ApiError::from_api_error_type(&IDEMPOTENT_STORE_CALL_ERROR_TYPE)
            .detail(Some("Could not process idempotent data in store"));
}
