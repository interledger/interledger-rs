/// APIs should implement their own `ApiErrorType`s to provide more detailed information
/// about what were the problem, for example, `JSON_SYNTAX_TYPE` or `ACCOUNT_NOT_FOUND_TYPE`.
mod error_types;
pub use error_types::*;

use chrono::{DateTime, Local};
use http::header::HeaderValue;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{ser::Serializer, Serialize};
use serde_json::error::Category;
use serde_json::{Map, Value};
use std::{
    error::Error as StdError,
    fmt::{self, Display},
};
use warp::{reject::Reject, reply::json, reply::Response, Rejection, Reply};

/// API error type prefix of problems.
/// This URL prefix is currently not published but we assume that in the future.
const ERROR_TYPE_PREFIX: &str = "https://errors.interledger.org/http-api";

/// This struct represents the fields defined in [RFC7807](https://tools.ietf.org/html/rfc7807).
/// The meaning of each field could be found at [Members of a Problem Details Object](https://tools.ietf.org/html/rfc7807#section-3.1) section.
/// ApiError implements Reply so that it could be used for responses.
#[derive(Clone, Debug, Serialize)]
pub struct ApiError {
    /// `type` is a URI which represents an error type. The URI should provide human-readable
    /// documents so that developers can solve the problem easily.
    #[serde(serialize_with = "serialize_type")]
    pub r#type: &'static ProblemType,
    /// `title` is a short, human-readable summary of the type.
    /// SHOULD NOT change from occurrence to occurrence of the problem, except for purposes
    /// of localization.
    pub title: &'static str,
    /// `status` is a HTTP status of the problem.
    #[serde(serialize_with = "serialize_status_code")]
    pub status: http::StatusCode,
    /// `detail` explains the problem in human-readable detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `instance` is a URI reference that identifies the specific occurrence of the problem.
    /// We should be careful of how we provide the URI because if it provides very detailed
    /// information about the error, it might expose some vulnerabilities of the node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// `extension_members` is a Map of JSON values which will be flatly injected into response
    /// JSONs. For example, if we specify `extension_members` like:
    /// ```JSON
    /// "invalid-params": [
    ///     { "name": "Username", "type": "missing" }
    /// ]
    /// ```
    /// then this map is merged into the response JSON and will look like:
    /// ```JSON
    /// {
    ///     "type": "about:blank",
    ///     "title": "Missing Fields Error",
    ///     "status": 400,
    ///     "detail": "foo bar",
    ///     "invalid-params": [
    ///         { "name": "Username", "type": "missing" }
    ///     ]
    /// }
    /// ```
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extension_members: Option<Map<String, Value>>,
}

/// Distinguishes between RFC7807 and Interledger API Errors
#[derive(Clone, Copy, Debug)]
pub enum ProblemType {
    /// `Default` is a [pre-defined value](https://tools.ietf.org/html/rfc7807#section-4.2) which is
    /// going to be serialized as `about:blank`.
    Default,
    /// InterledgerHttpApi is a API specific error type which is going to be serialized like
    /// `https://errors.interledger.org/http-api/foo-bar`. Variant means path, in the example,
    /// `foo-bar` is the path.
    InterledgerHttpApi(&'static str),
}

/// Error type used as a basis for creating Warp-compatible Errors
#[derive(Clone, Copy, Debug)]
pub struct ApiErrorType {
    /// Interledger or RFC7807 Error
    pub r#type: &'static ProblemType,
    /// The Title to be used for the error page
    pub title: &'static str,
    /// The HTTP Status Code for the error
    pub status: http::StatusCode,
}

// This should be OK because serde serializer MUST be `fn<S>(&T, S)`
#[allow(clippy::trivially_copy_pass_by_ref)]
fn serialize_status_code<S>(status: &http::StatusCode, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_u16(status.as_u16())
}

fn serialize_type<S>(r#type: &ProblemType, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match r#type {
        ProblemType::Default => s.serialize_str("about:blank"),
        ProblemType::InterledgerHttpApi(custom_type) => {
            s.serialize_str(&format!("{}/{}", ERROR_TYPE_PREFIX, custom_type))
        }
    }
}

impl ApiError {
    /// Constructs an API Error from a [ApiErrorType](./struct.ApiErrorType.html)
    pub fn from_api_error_type(problem_type: &ApiErrorType) -> Self {
        ApiError {
            r#type: problem_type.r#type,
            title: problem_type.title,
            status: problem_type.status,
            detail: None,
            instance: None,
            extension_members: Some(ApiError::merge_default_extension_members(None)),
        }
    }

    // Note that we should basically avoid using the following default errors because
    // we should provide more detailed information for developers

    #[allow(dead_code)]
    /// Returns a Bad Request [ApiError](./struct.ApiError.html)
    pub fn bad_request() -> Self {
        ApiError::from_api_error_type(&DEFAULT_BAD_REQUEST_TYPE)
    }

    /// Returns an Internal Server Error [ApiError](./struct.ApiError.html)
    pub fn internal_server_error() -> Self {
        ApiError::from_api_error_type(&DEFAULT_INTERNAL_SERVER_ERROR_TYPE)
    }

    /// Returns an Unauthorized [ApiError](./struct.ApiError.html)
    pub fn unauthorized() -> Self {
        ApiError::from_api_error_type(&DEFAULT_UNAUTHORIZED_TYPE)
    }

    #[allow(dead_code)]
    /// Returns an Error Not Found [ApiError](./struct.ApiError.html)
    pub fn not_found() -> Self {
        ApiError::from_api_error_type(&DEFAULT_NOT_FOUND_TYPE)
    }

    #[allow(dead_code)]
    /// Returns a Method Not Found [ApiError](./struct.ApiError.html)
    pub fn method_not_allowed() -> Self {
        ApiError::from_api_error_type(&DEFAULT_METHOD_NOT_ALLOWED_TYPE)
    }

    /// Returns an Account not Found [ApiError](./struct.ApiError.html)
    pub fn account_not_found() -> Self {
        ApiError::from_api_error_type(&ACCOUNT_NOT_FOUND_TYPE)
            .detail("Username was not found.".to_owned())
    }

    #[allow(dead_code)]
    /// Returns an Idempotency Conflict [ApiError](./struct.ApiError.html)
    /// via the [default idempotency conflict ApiErrorType](./error_types/constant.DEFAULT_IDEMPOTENT_CONFLICT_TYPE.html)
    pub fn idempotency_conflict() -> Self {
        ApiError::from_api_error_type(&DEFAULT_IDEMPOTENT_CONFLICT_TYPE)
    }

    #[allow(dead_code)]
    /// Returns an Conflict [ApiError](./struct.ApiError.html)
    /// via the [default conflict ApiErrorType](./error_types/constant.DEFAULT_CONFLICT_TYPE.html)
    pub fn conflict() -> Self {
        ApiError::from_api_error_type(&DEFAULT_CONFLICT_TYPE)
    }

    /// Returns an Invalid Account Id [ApiError](./struct.ApiError.html)
    pub fn invalid_account_id(invalid_account_id: Option<&str>) -> Self {
        let detail = match invalid_account_id {
            Some(invalid_account_id) => match invalid_account_id.len() {
                0 => "Account ID is empty".to_owned(),
                _ => format!("{} is an invalid account ID", invalid_account_id),
            },
            None => "Invalid string was given as an account ID".to_owned(),
        };
        ApiError::from_api_error_type(&INVALID_ACCOUNT_ID_TYPE).detail(detail)
    }

    /// Returns an Invalid ILP over HTTP [ApiError](./struct.ApiError.html)
    pub fn invalid_ilp_packet() -> Self {
        ApiError::from_api_error_type(&INVALID_ILP_PACKET_TYPE)
    }

    /// Sets the [`detail`](./struct.ApiError.html#structfield.detail) field
    pub fn detail<T>(mut self, detail: T) -> Self
    where
        T: Into<String>,
    {
        self.detail = Some(detail.into());
        self
    }

    #[allow(dead_code)]
    /// Sets the [`instance`](./struct.ApiError.html#structfield.instance) field
    pub fn instance<T>(mut self, instance: T) -> Self
    where
        T: Into<String>,
    {
        self.instance = Some(instance.into());
        self
    }

    /// Sets the [`extension_members`](./struct.ApiError.html#structfield.extension_members) field
    pub fn extension_members(mut self, extension_members: Map<String, Value>) -> Self {
        self.extension_members = Some(extension_members);
        self
    }

    fn get_base_extension_members() -> Map<String, Value> {
        // TODO Should implement request wide time
        let datetime: DateTime<Local> = Local::now();
        let mut map = serde_json::Map::new();
        map.insert("datetime".to_owned(), Value::from(datetime.to_rfc3339()));
        map
    }

    fn merge_default_extension_members(
        extension_members: Option<Map<String, Value>>,
    ) -> Map<String, Value> {
        let mut merged_extension_members = ApiError::get_base_extension_members();
        if let Some(map) = extension_members {
            for (k, v) in map {
                merged_extension_members.insert(k, v);
            }
        }
        merged_extension_members
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_fmt(format_args!("{:?}", self))
    }
}

impl Reply for ApiError {
    fn into_response(self) -> Response {
        let res = json(&self);
        let mut res = res.into_response();
        *res.status_mut() = self.status;
        res.headers_mut().insert(
            "Content-Type",
            HeaderValue::from_static("application/problem+json"),
        );
        res
    }
}

impl StdError for ApiError {}

impl Reject for ApiError {}

static MISSING_FIELD_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new("missing field `(.*)`").unwrap());

#[derive(Clone, Debug)]
pub struct JsonDeserializeError {
    pub category: Category,
    pub detail: String,
    pub path: serde_path_to_error::Path,
}

impl StdError for JsonDeserializeError {}
impl Reject for JsonDeserializeError {}

impl Display for JsonDeserializeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_fmt(format_args!("{:?}", self))
    }
}

impl Reply for JsonDeserializeError {
    fn into_response(self) -> Response {
        let mut extension_members = Map::new();

        // invalid-params should be a plural form even if it is always an array with a single value
        // for the future extendability.

        // if `path` has segments and the first value is not Unknown
        if let Some(segment) = self.path.iter().next() {
            match segment {
                serde_path_to_error::Segment::Unknown => {}
                _ => {
                    let invalid_params = serde_json::json!([ { "name": self.path.to_string() } ]);
                    extension_members.insert("invalid-params".to_string(), invalid_params);
                }
            }
        }

        // if detail contains missing field error
        // it seems that there is no way to handle this cleanly
        if let Some(captures) = MISSING_FIELD_REGEX.captures(&self.detail) {
            if let Some(r#match) = captures.get(1) {
                let invalid_params =
                    serde_json::json!([ { "name": r#match.as_str(), "type": "missing" } ]);
                extension_members.insert("invalid-params".to_string(), invalid_params);
            }
        }

        let api_error_type = match self.category {
            Category::Syntax => &JSON_SYNTAX_TYPE,
            Category::Data => &JSON_DATA_TYPE,
            Category::Eof => &JSON_EOF_TYPE,
            Category::Io => &JSON_IO_TYPE,
        };
        let detail = self.detail;

        let mut error = ApiError::from_api_error_type(api_error_type).detail(detail);

        if extension_members.keys().len() > 0 {
            error = error.extension_members(extension_members);
        }

        error.into_response()
    }
}

// Receives `ApiError`s and `JsonDeserializeError` and return it in the RFC7807 format.
pub async fn default_rejection_handler(err: warp::Rejection) -> Result<impl Reply, Rejection> {
    if let Some(api_error) = err.find::<ApiError>() {
        Ok(api_error.clone().into_response())
    } else if let Some(json_error) = err.find::<JsonDeserializeError>() {
        Ok(json_error.clone().into_response())
    } else if err.find::<warp::reject::MethodNotAllowed>().is_some() {
        Ok(ApiError::from_api_error_type(&DEFAULT_METHOD_NOT_ALLOWED_TYPE).into_response())
    } else {
        Err(err)
    }
}
