// APIs should implement their own `ApiErrorType`s to provide more detailed information
// about what were the problem, for example, `JSON_SYNTAX_TYPE` or `ACCOUNT_NOT_FOUND_TYPE`.

use super::{ApiErrorType, ProblemType};

// Default errors
#[allow(dead_code)]
pub const DEFAULT_BAD_REQUEST_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Bad Request",
    status: http::StatusCode::BAD_REQUEST,
};
pub const DEFAULT_INTERNAL_SERVER_ERROR_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Internal Server Error",
    status: http::StatusCode::INTERNAL_SERVER_ERROR,
};
pub const DEFAULT_UNAUTHORIZED_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Unauthorized",
    status: http::StatusCode::UNAUTHORIZED,
};
pub const DEFAULT_NOT_FOUND_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Not Found",
    status: http::StatusCode::NOT_FOUND,
};
pub const DEFAULT_METHOD_NOT_ALLOWED_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Method Not Allowed",
    status: http::StatusCode::METHOD_NOT_ALLOWED,
};
pub const DEFAULT_IDEMPOTENT_CONFLICT_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::Default,
    title: "Provided idempotency key is tied to other input",
    status: http::StatusCode::CONFLICT,
};

// ILP over HTTP specific errors
pub const INVALID_ILP_PACKET_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("ilp-over-http/invalid-packet"),
    title: "Invalid Packet",
    status: http::StatusCode::BAD_REQUEST,
};

// JSON deserialization errors
pub const JSON_SYNTAX_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("json-syntax"),
    title: "JSON Syntax Error",
    status: http::StatusCode::BAD_REQUEST,
};
pub const JSON_DATA_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("json-data"),
    title: "JSON Data Error",
    status: http::StatusCode::BAD_REQUEST,
};
pub const UNKNOWN_JSON_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("json-unknown"),
    title: "Unknown JSON Error",
    status: http::StatusCode::BAD_REQUEST,
};

// Account specific errors
pub const ACCOUNT_NOT_FOUND_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("accounts/account-not-found"),
    title: "Account Not Found",
    status: http::StatusCode::NOT_FOUND,
};

// Node settings specific errors
pub const INVALID_ACCOUNT_ID_TYPE: ApiErrorType = ApiErrorType {
    r#type: &ProblemType::InterledgerHttpApi("settings/invalid-account-id"),
    title: "Invalid Account Id",
    status: http::StatusCode::BAD_REQUEST,
};
