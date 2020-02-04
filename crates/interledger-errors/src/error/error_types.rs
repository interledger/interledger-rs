use super::{ApiError, ApiErrorType, ProblemType};
use http::StatusCode;
use once_cell::sync::Lazy;

// Common HTTP errors

#[allow(dead_code)]
/// 400 Bad Request HTTP Status Code
pub const DEFAULT_BAD_REQUEST_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Bad Request",
    status: StatusCode::BAD_REQUEST,
};

/// 500 Internal Server Error HTTP Status Code
pub const DEFAULT_INTERNAL_SERVER_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Internal Server Error",
    status: StatusCode::INTERNAL_SERVER_ERROR,
};

/// 401 Unauthorized HTTP Status Code
pub const DEFAULT_UNAUTHORIZED_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Unauthorized",
    status: StatusCode::UNAUTHORIZED,
};

/// 404 Not Found HTTP Status Code
pub const DEFAULT_NOT_FOUND_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Not Found",
    status: StatusCode::NOT_FOUND,
};

/// 405 Method Not Allowed HTTP Status Code
pub const DEFAULT_METHOD_NOT_ALLOWED_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Method Not Allowed",
    status: StatusCode::METHOD_NOT_ALLOWED,
};

/// 409 Conflict HTTP Status Code (used for conflicts)
pub const DEFAULT_CONFLICT_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Provided resource already exists",
    status: StatusCode::CONFLICT,
};

/// 409 Conflict HTTP Status Code (used for Idempotency Conflicts)
pub const DEFAULT_IDEMPOTENT_CONFLICT_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Provided idempotency key is tied to other input",
    status: StatusCode::CONFLICT,
};

// ILP over HTTP specific errors

/// ILP over HTTP invalid packet error type  (400 Bad Request)
pub const INVALID_ILP_PACKET_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("ilp-over-http/invalid-packet"),
    title: "Invalid Packet",
    status: StatusCode::BAD_REQUEST,
};

/// Wrong JSON syntax error type (400 Bad Request)
pub const JSON_SYNTAX_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("json-syntax"),
    title: "JSON Syntax Error",
    status: StatusCode::BAD_REQUEST,
};

/// Wrong JSON data error type (400 Bad Request)
pub const JSON_DATA_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("json-data"),
    title: "JSON Data Error",
    status: StatusCode::BAD_REQUEST,
};

/// JSON EOF error type (400 Bad Request)
pub const JSON_EOF_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("json-eof"),
    title: "JSON Unexpected EOF",
    status: StatusCode::BAD_REQUEST,
};

/// JSON IO error type (400 Bad Request)
pub const JSON_IO_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("json-io"),
    title: "JSON IO Error",
    status: StatusCode::BAD_REQUEST,
};

// Account specific errors

/// Account Not Found error type (404 Not Found)
pub const ACCOUNT_NOT_FOUND_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("accounts/account-not-found"),
    title: "Account Not Found",
    status: StatusCode::NOT_FOUND,
};

/// Invalid Account Id error type (400 Bad Request)
pub const INVALID_ACCOUNT_ID_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("settings/invalid-account-id"),
    title: "Invalid Account Id",
    status: StatusCode::BAD_REQUEST,
};

// String used for idempotency errors
pub static IDEMPOTENCY_CONFLICT_ERR: &str = "Provided idempotency key is tied to other input";

/// 409 Conflict HTTP Status Code (used for Idempotency Conflicts)
// TODO: Remove this since it is a duplicate of DEFAULT_IDEMPOTENT_CONFLICT_TYPE
pub const IDEMPOTENT_STORE_CALL_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Store idempotency error",
    status: StatusCode::CONFLICT,
};

/// Error which must be returned when the same idempotency key is
/// used for more than 1 input
pub static IDEMPOTENT_STORE_CALL_ERROR: Lazy<ApiError> = Lazy::new(|| {
    ApiError::from_api_error_type(&IDEMPOTENT_STORE_CALL_ERROR_TYPE)
        .detail("Could not process idempotent data in store")
});
