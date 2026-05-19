//! Property-based tests for API error response structure.
//!
//! **Validates: Requirements 9.4**
//!
//! Property 15: API Error Response Structure
//!
//! For any API error (any status code, any error code, any message), the JSON
//! response always contains exactly three fields: `error_code` (non-empty string),
//! `message` (non-empty string), and `request_id` (valid UUID string).
//! Additionally, 429 responses always include a Retry-After header with a
//! positive integer value.

use api_server::ApiError;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use http_body_util::BodyExt;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use uuid::Uuid;

// ============================================================================
// Strategies for generating arbitrary API error inputs
// ============================================================================

/// Generate an arbitrary HTTP status code from the error range (4xx and 5xx).
fn arb_status_code() -> impl Strategy<Value = StatusCode> {
    prop_oneof![
        Just(StatusCode::BAD_REQUEST),
        Just(StatusCode::UNAUTHORIZED),
        Just(StatusCode::FORBIDDEN),
        Just(StatusCode::NOT_FOUND),
        Just(StatusCode::METHOD_NOT_ALLOWED),
        Just(StatusCode::CONFLICT),
        Just(StatusCode::GONE),
        Just(StatusCode::UNPROCESSABLE_ENTITY),
        Just(StatusCode::TOO_MANY_REQUESTS),
        Just(StatusCode::INTERNAL_SERVER_ERROR),
        Just(StatusCode::BAD_GATEWAY),
        Just(StatusCode::SERVICE_UNAVAILABLE),
        Just(StatusCode::GATEWAY_TIMEOUT),
    ]
}

/// Generate a non-empty error code string (machine-readable identifier).
fn arb_error_code() -> impl Strategy<Value = String> {
    "[A-Z][A-Z_]{2,30}"
}

/// Generate a non-empty human-readable error message.
fn arb_message() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,!?:;'-]{1,200}"
}

/// Generate a valid UUID string for request IDs.
fn arb_request_id() -> impl Strategy<Value = String> {
    Just(()).prop_map(|_| Uuid::new_v4().to_string())
}

/// Generate a positive retry-after value in seconds.
fn arb_retry_after() -> impl Strategy<Value = u64> {
    1..=3600u64
}

// ============================================================================
// Property 15: API Error Response Structure
//
// For any API error (any status code, any error code, any message), the JSON
// response always contains exactly three fields: `error_code` (non-empty string),
// `message` (non-empty string), and `request_id` (valid UUID string).
// Additionally, 429 responses always include a Retry-After header with a
// positive integer value.
//
// **Validates: Requirements 9.4**
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// **Validates: Requirements 9.4**
    ///
    /// Property 15a: For any arbitrary ApiError constructed with any status code,
    /// error code, and message, the JSON response body always contains exactly
    /// three fields: `error_code`, `message`, and `request_id`, all non-empty strings,
    /// and `request_id` is a valid UUID.
    #[test]
    fn prop_error_response_has_required_fields(
        status in arb_status_code(),
        error_code in arb_error_code(),
        message in arb_message(),
        request_id in arb_request_id(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let api_error = ApiError::with_request_id(status, &error_code, &message, request_id.clone());
            let response = api_error.into_response();

            // Extract the response body
            let body_bytes = response
                .into_body()
                .collect()
                .await
                .map_err(|e| TestCaseError::fail(format!("Failed to read body: {}", e)))?
                .to_bytes();

            let json: serde_json::Value = serde_json::from_slice(&body_bytes)
                .map_err(|e| TestCaseError::fail(format!("Failed to parse JSON: {}", e)))?;

            // Verify the response is a JSON object
            let obj = json.as_object().ok_or_else(|| {
                TestCaseError::fail("Response body is not a JSON object")
            })?;

            // Verify exactly three fields exist
            prop_assert_eq!(
                obj.len(), 3,
                "Response should have exactly 3 fields, got {}: {:?}",
                obj.len(), obj.keys().collect::<Vec<_>>()
            );

            // Verify error_code field exists and is a non-empty string
            let resp_error_code = obj.get("error_code")
                .ok_or_else(|| TestCaseError::fail("Missing 'error_code' field"))?
                .as_str()
                .ok_or_else(|| TestCaseError::fail("'error_code' is not a string"))?;
            prop_assert!(!resp_error_code.is_empty(), "error_code must be non-empty");

            // Verify message field exists and is a non-empty string
            let resp_message = obj.get("message")
                .ok_or_else(|| TestCaseError::fail("Missing 'message' field"))?
                .as_str()
                .ok_or_else(|| TestCaseError::fail("'message' is not a string"))?;
            prop_assert!(!resp_message.is_empty(), "message must be non-empty");

            // Verify request_id field exists and is a valid UUID string
            let resp_request_id = obj.get("request_id")
                .ok_or_else(|| TestCaseError::fail("Missing 'request_id' field"))?
                .as_str()
                .ok_or_else(|| TestCaseError::fail("'request_id' is not a string"))?;
            prop_assert!(!resp_request_id.is_empty(), "request_id must be non-empty");

            // Verify request_id is a valid UUID
            Uuid::parse_str(resp_request_id).map_err(|e| {
                TestCaseError::fail(format!(
                    "request_id '{}' is not a valid UUID: {}", resp_request_id, e
                ))
            })?;

            // Verify the values match what we provided
            prop_assert_eq!(resp_error_code, error_code.as_str());
            prop_assert_eq!(resp_message, message.as_str());
            prop_assert_eq!(resp_request_id, request_id.as_str());

            Ok(())
        });
        result?;
    }

    /// **Validates: Requirements 9.4**
    ///
    /// Property 15b: For any 429 (Too Many Requests) response, the Retry-After
    /// header is always present with a positive integer value.
    #[test]
    fn prop_rate_limited_response_has_retry_after_header(
        retry_after in arb_retry_after(),
        request_id in arb_request_id(),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let api_error = ApiError::rate_limited(retry_after, request_id.clone());
            let response = api_error.into_response();

            // Verify status code is 429
            prop_assert_eq!(
                response.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "Rate limited error should return 429 status"
            );

            // Verify Retry-After header is present
            let retry_header = response.headers().get("Retry-After")
                .ok_or_else(|| TestCaseError::fail("Missing Retry-After header on 429 response"))?;

            // Verify Retry-After header is a valid positive integer
            let retry_value: u64 = retry_header
                .to_str()
                .map_err(|e| TestCaseError::fail(format!("Retry-After header is not valid UTF-8: {}", e)))?
                .parse()
                .map_err(|e| TestCaseError::fail(format!("Retry-After header is not a valid integer: {}", e)))?;

            prop_assert!(retry_value > 0, "Retry-After must be a positive integer, got {}", retry_value);
            prop_assert_eq!(retry_value, retry_after, "Retry-After value should match the configured value");

            // Also verify the body has the correct structure
            let body_bytes = response
                .into_body()
                .collect()
                .await
                .map_err(|e| TestCaseError::fail(format!("Failed to read body: {}", e)))?
                .to_bytes();

            let json: serde_json::Value = serde_json::from_slice(&body_bytes)
                .map_err(|e| TestCaseError::fail(format!("Failed to parse JSON: {}", e)))?;

            let obj = json.as_object().ok_or_else(|| {
                TestCaseError::fail("Response body is not a JSON object")
            })?;

            // Verify exactly three fields
            prop_assert_eq!(obj.len(), 3, "Response should have exactly 3 fields");

            // Verify error_code is non-empty
            let error_code = obj.get("error_code")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TestCaseError::fail("Missing or invalid 'error_code'"))?;
            prop_assert!(!error_code.is_empty(), "error_code must be non-empty");

            // Verify message is non-empty
            let message = obj.get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TestCaseError::fail("Missing or invalid 'message'"))?;
            prop_assert!(!message.is_empty(), "message must be non-empty");

            // Verify request_id is a valid UUID
            let req_id = obj.get("request_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TestCaseError::fail("Missing or invalid 'request_id'"))?;
            Uuid::parse_str(req_id).map_err(|e| {
                TestCaseError::fail(format!("request_id is not a valid UUID: {}", e))
            })?;

            Ok(())
        });
        result?;
    }

    /// **Validates: Requirements 9.4**
    ///
    /// Property 15c: All convenience constructors (bad_request, unauthorized,
    /// forbidden, not_found, rate_limited, internal) produce responses with
    /// the correct three-field JSON structure and valid UUID request_id.
    #[test]
    fn prop_convenience_constructors_produce_valid_responses(
        message in arb_message(),
        request_id in arb_request_id(),
        retry_after in arb_retry_after(),
        constructor_idx in 0..6usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result: Result<(), TestCaseError> = rt.block_on(async {
            let api_error = match constructor_idx {
                0 => ApiError::bad_request(&message, request_id.clone()),
                1 => ApiError::unauthorized(request_id.clone()),
                2 => ApiError::forbidden(&message, request_id.clone()),
                3 => ApiError::not_found(&message, request_id.clone()),
                4 => ApiError::rate_limited(retry_after, request_id.clone()),
                5 => ApiError::internal(&message, request_id.clone()),
                _ => unreachable!(),
            };

            let response = api_error.into_response();

            // Extract body
            let body_bytes = response
                .into_body()
                .collect()
                .await
                .map_err(|e| TestCaseError::fail(format!("Failed to read body: {}", e)))?
                .to_bytes();

            let json: serde_json::Value = serde_json::from_slice(&body_bytes)
                .map_err(|e| TestCaseError::fail(format!("Failed to parse JSON: {}", e)))?;

            let obj = json.as_object().ok_or_else(|| {
                TestCaseError::fail("Response body is not a JSON object")
            })?;

            // Verify exactly three fields
            prop_assert_eq!(
                obj.len(), 3,
                "Constructor {} should produce exactly 3 fields, got {}: {:?}",
                constructor_idx, obj.len(), obj.keys().collect::<Vec<_>>()
            );

            // Verify error_code is a non-empty string
            let error_code = obj.get("error_code")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TestCaseError::fail("Missing or invalid 'error_code'"))?;
            prop_assert!(!error_code.is_empty(),
                "Constructor {} error_code must be non-empty", constructor_idx);

            // Verify message is a non-empty string
            let msg = obj.get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TestCaseError::fail("Missing or invalid 'message'"))?;
            prop_assert!(!msg.is_empty(),
                "Constructor {} message must be non-empty", constructor_idx);

            // Verify request_id is a valid UUID
            let req_id = obj.get("request_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| TestCaseError::fail("Missing or invalid 'request_id'"))?;
            prop_assert!(!req_id.is_empty(),
                "Constructor {} request_id must be non-empty", constructor_idx);
            Uuid::parse_str(req_id).map_err(|e| {
                TestCaseError::fail(format!(
                    "Constructor {} request_id '{}' is not a valid UUID: {}",
                    constructor_idx, req_id, e
                ))
            })?;

            Ok(())
        });
        result?;
    }
}
