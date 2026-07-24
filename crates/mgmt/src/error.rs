//! Error type for management API calls, mapping HTTP statuses to the
//! conditions users actually need to distinguish.

use reqwest::StatusCode;

#[derive(Debug, thiserror::Error)]
pub enum MgmtError {
    #[error(
        "unauthorized: check the SAS key or credential (clock skew also invalidates SAS tokens)"
    )]
    Unauthorized { detail: String },
    #[error("forbidden: the credential is valid but lacks Manage rights")]
    Forbidden { detail: String },
    #[error("'{path}' was not found")]
    NotFound { path: String },
    #[error("conflict: the entity already exists or a conflicting operation is in progress")]
    Conflict { detail: String },
    #[error("the request was rejected as invalid: {detail}")]
    BadRequest { detail: String },
    #[error("the service is throttling requests; retry later")]
    Throttled { detail: String },
    #[error("the service returned HTTP {status}: {detail}")]
    Server { status: u16, detail: String },
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("could not parse the service response: {0}")]
    Xml(String),
    #[error("invalid management URL: {0}")]
    Url(#[from] url::ParseError),
}

impl MgmtError {
    /// Map a non-success HTTP response to an error. `detail` is the raw
    /// response body (the service returns an XML `<Error>` document).
    pub(crate) fn from_status(status: StatusCode, path: &str, detail: String) -> Self {
        match status {
            StatusCode::UNAUTHORIZED => Self::Unauthorized { detail },
            StatusCode::FORBIDDEN => Self::Forbidden { detail },
            StatusCode::NOT_FOUND => Self::NotFound {
                path: path.to_owned(),
            },
            StatusCode::CONFLICT => Self::Conflict { detail },
            StatusCode::BAD_REQUEST => Self::BadRequest { detail },
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
                Self::Throttled { detail }
            }
            other => Self::Server {
                status: other.as_u16(),
                detail,
            },
        }
    }

    /// Whether retrying the same request may succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Throttled { .. } => true,
            Self::Server { status, .. } => *status >= 500,
            Self::Http(e) => e.is_timeout() || e.is_connect(),
            _ => false,
        }
    }
}
