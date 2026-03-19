// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

use reqwest::Method;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::error::Error;

pub struct CbcClient {
    inner: reqwest::Client,
    base_url: Url,
    debug: bool,
}

impl CbcClient {
    /// Create an authenticated client.
    pub fn new(host: &str, token: &str, debug: bool) -> Result<Self, Error> {
        let base_url = parse_base_url(host)?;

        let mut headers = HeaderMap::new();
        let auth_value = format!("Bearer {token}");
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value)
                .map_err(|e| Error::Config(format!("invalid token: {e}")))?,
        );

        let inner = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| Error::Connection(format!("cannot build HTTP client: {e}")))?;

        Ok(Self {
            inner,
            base_url,
            debug,
        })
    }

    /// Create an unauthenticated client (for pre-login health checks).
    pub fn unauthenticated(host: &str, debug: bool) -> Result<Self, Error> {
        let base_url = parse_base_url(host)?;

        let inner = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Connection(format!("cannot build HTTP client: {e}")))?;

        Ok(Self {
            inner,
            base_url,
            debug,
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        self.request::<T>(Method::GET, path, Option::<&()>::None)
            .await
    }

    /// Return a raw `RequestBuilder` for the given method and API path.
    ///
    /// Useful when the caller needs to customise the request (e.g. SSE
    /// streaming) instead of going through the generic JSON helpers.
    pub fn request_builder(
        &self,
        method: Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let url = self
            .base_url
            .join(path)
            .map_err(|e| Error::Connection(format!("invalid path '{path}': {e}")))?;

        if self.debug {
            eprintln!("{method} {url}");
        }

        Ok(self.inner.request(method, url))
    }

    /// Send a GET request and return the raw response for streaming.
    pub async fn get_stream(&self, path: &str) -> Result<reqwest::Response, Error> {
        let url = self
            .base_url
            .join(path)
            .map_err(|e| Error::Connection(format!("invalid path '{path}': {e}")))?;

        if self.debug {
            eprintln!("GET {url}");
        }

        let resp = self
            .inner
            .get(url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        let status = resp.status();

        if self.debug {
            eprintln!("  -> {status}");
        }

        if status.is_success() {
            Ok(resp)
        } else {
            let status_code = status.as_u16();
            let text = resp.text().await.unwrap_or_default();

            let message = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("error")?.as_str().map(String::from))
                .unwrap_or(text);

            Err(Error::Api {
                status: status_code,
                message,
            })
        }
    }

    pub async fn post<B: Serialize + Sync, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, Error> {
        self.request::<T>(Method::POST, path, Some(body)).await
    }

    pub async fn put_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &(impl Serialize + Sync),
    ) -> Result<T, Error> {
        self.request::<T>(Method::PUT, path, Some(body)).await
    }

    pub async fn put_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        self.request::<T>(Method::PUT, path, Option::<&()>::None)
            .await
    }

    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        self.request::<T>(Method::DELETE, path, Option::<&()>::None)
            .await
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&(impl Serialize + Sync)>,
    ) -> Result<T, Error> {
        let url = self
            .base_url
            .join(path)
            .map_err(|e| Error::Connection(format!("invalid path '{path}': {e}")))?;

        if self.debug {
            eprintln!("{method} {url}");
        }

        let mut req = self.inner.request(method.clone(), url);
        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        let status = resp.status();

        if self.debug {
            eprintln!("  -> {status}");
        }

        if status.is_success() {
            resp.json::<T>()
                .await
                .map_err(|e| Error::Other(format!("cannot decode response: {e}")))
        } else {
            let status_code = status.as_u16();
            let text = resp.text().await.unwrap_or_default();

            // Try to extract a structured error message.
            let message = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("error")?.as_str().map(String::from))
                .unwrap_or(text);

            Err(Error::Api {
                status: status_code,
                message,
            })
        }
    }
}

/// Ensure the host URL ends with `/api/`.
fn parse_base_url(host: &str) -> Result<Url, Error> {
    let mut s = host.to_string();
    if !s.ends_with('/') {
        s.push('/');
    }
    let mut url =
        Url::parse(&s).map_err(|e| Error::Config(format!("invalid host URL '{host}': {e}")))?;
    // Append "api/" to the path so all relative joins resolve under /api/.
    url.set_path(&format!("{}api/", url.path()));
    Ok(url)
}
