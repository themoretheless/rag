//! Shared fake transport for HTTP loader tests.

use crate::gateway::{GatewayClient, Request, Response};
use std::sync::Mutex;

pub(crate) struct FakeGateway {
    pub(crate) response: Response,
    pub(crate) requests: Mutex<Vec<Request>>,
}

impl FakeGateway {
    pub(crate) fn ok(body: &str) -> Self {
        Self {
            response: Response {
                status: 200,
                body: body.to_string(),
            },
            requests: Mutex::new(Vec::new()),
        }
    }
}

impl GatewayClient for FakeGateway {
    fn execute(&self, request: Request) -> Result<Response, String> {
        self.requests.lock().unwrap().push(request);
        Ok(self.response.clone())
    }
}
