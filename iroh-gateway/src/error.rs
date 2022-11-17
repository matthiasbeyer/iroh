use axum::{
    body::BoxBody,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use http::{HeaderMap, HeaderValue};
use opentelemetry::trace::TraceId;
use serde_json::json;

use crate::constants::HEADER_X_TRACE_ID;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    RpcTypes(#[from] iroh_rpc_types::error::Error),

    #[error(transparent)]
    Resolver(#[from] iroh_resolver::error::Error),

    #[error(transparent)]
    Car(#[from] iroh_car::Error),

    #[error(transparent)]
    Axum(#[from] axum::Error),

    #[error(transparent)]
    Http(#[from] http::Error),

    #[error("root cid not found")]
    RootCidNotFound,

    #[error("can not derive rpc_addr for mem addr")]
    CanNotDeriveRpcAddrForMemAddr,

    #[error("invalid rpc_addr")]
    InvalidRpcAddr,

    #[error("gateway_error({}): {})", .status_code, .message)]
    GatewayError {
        status_code: StatusCode,
        message: String,
        trace_id: TraceId,
        method: Option<http::Method>,
    },
}

impl Error {
    pub fn with_method(self, m: http::Method) -> Self {
        if let Self::GatewayError {
            status_code,
            message,
            trace_id,
            ..
        } = self
        {
            Self::GatewayError {
                method: Some(m),
                status_code,
                message,
                trace_id,
            }
        } else {
            self
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        match self {
            Self::RpcTypes(_) => {
                unimplemented!()
            },

            Self::Resolver(_) => {
                unimplemented!()
            },

            Self::Car(_) => {
                unimplemented!()
            },

            Self::Axum(_) => {
                unimplemented!()
            },

            Self::Http(_) => {
                unimplemented!()
            },

            Self::RootCidNotFound => {
                unimplemented!()
            },

            Self::CanNotDeriveRpcAddrForMemAddr => {
                unimplemented!()
            },

            Self::InvalidRpcAddr => {
                unimplemented!()
            },

            Error::GatewayError {
                status_code,
                message,
                trace_id,
                method,
            } => {
                let mut headers = HeaderMap::new();
                if trace_id != TraceId::INVALID {
                    headers.insert(
                        &HEADER_X_TRACE_ID,
                        HeaderValue::from_str(&trace_id.to_string()).unwrap(),
                    );
                }
                match method {
                    Some(http::Method::HEAD) => {
                        let mut rb = Response::builder().status(status_code);
                        let rh = rb.headers_mut().unwrap();
                        rh.extend(headers);
                        rb.body(BoxBody::default()).unwrap()
                    }
                    _ => {
                        let body = if trace_id != TraceId::INVALID {
                            axum::Json(json!({
                                "code": status_code.as_u16(),
                                "success": false,
                                "message": message,
                                "trace_id": trace_id.to_string(),
                            }))
                        } else {
                            axum::Json(json!({
                                "code": status_code.as_u16(),
                                "success": false,
                                "message": message,
                            }))
                        };
                        let mut res = body.into_response();
                        res.headers_mut().extend(headers);
                        let status = res.status_mut();
                        *status = status_code;
                        res
                    }
                }
            }
        }
    }
}
