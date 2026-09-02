//! HTTP transport boundary for the native UI.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Delete,
    Get,
    Post,
    Put,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub body: Option<String>,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub trait GatewayClient {
    fn execute(&self, request: Request) -> Result<Response, String>;
}

pub struct ReqwestGatewayClient {
    client: reqwest::blocking::Client,
}

impl ReqwestGatewayClient {
    pub fn new(timeout: Duration) -> Result<Self, String> {
        reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map(|client| Self { client })
            .map_err(|error| format!("http client: {error}"))
    }
}

impl GatewayClient for ReqwestGatewayClient {
    fn execute(&self, request: Request) -> Result<Response, String> {
        let mut builder = match request.method {
            Method::Delete => self.client.delete(&request.url),
            Method::Get => self.client.get(&request.url),
            Method::Post => self.client.post(&request.url),
            Method::Put => self.client.put(&request.url),
        };
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder
                .header("Content-Type", "application/json")
                .body(body);
        }
        let response = builder.send().map_err(|error| {
            format!("HTTP {:?} {} failed: {error}", request.method, request.url)
        })?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .map_err(|error| format!("read HTTP response from {}: {error}", request.url))?;
        Ok(Response { status, body })
    }
}
