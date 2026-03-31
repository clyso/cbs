// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

use std::sync::Arc;

use rand::Rng;
use tokio_tungstenite::tungstenite;
use tungstenite::client::IntoClientRequest;

use crate::config::ResolvedWorkerConfig;
use crate::signal::ShutdownState;
use crate::ws::handler;

/// A WebSocket stream over a TLS or plain TCP connection.
pub type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

// ---------------------------------------------------------------------------
// Connect
// ---------------------------------------------------------------------------

/// Establish a WebSocket connection to the server with the `Authorization`
/// header set from `config.api_key`.
///
/// When `config.dev_mode` is true, TLS certificate verification is disabled
/// so the worker can connect through reverse-proxies with self-signed
/// certificates.
async fn connect(config: &ResolvedWorkerConfig) -> Result<WsStream, ConnectionError> {
    let mut request = config
        .server_url
        .as_str()
        .into_client_request()
        .map_err(ConnectionError::Request)?;

    let header_value = http::HeaderValue::from_str(&format!("Bearer {}", config.api_key))
        .map_err(|e| ConnectionError::InvalidHeader(e.to_string()))?;
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, header_value);

    let connector = if config.dev_mode {
        Some(tokio_tungstenite::Connector::Rustls(Arc::new(
            dev_tls_config(),
        )))
    } else {
        None
    };

    let (stream, _response) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector)
            .await
            .map_err(ConnectionError::WebSocket)?;

    Ok(stream)
}

/// Build a rustls `ClientConfig` that accepts any server certificate.
///
/// Used only in development mode (`CBSD_DEV` is set) where the
/// reverse-proxy presents self-signed certificates.
fn dev_tls_config() -> rustls::ClientConfig {
    rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier))
        .with_no_client_auth()
}

/// A [`rustls`] certificate verifier that accepts every certificate.
#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _server_name: &rustls_pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

// ---------------------------------------------------------------------------
// Reconnect loop
// ---------------------------------------------------------------------------

/// Run the worker's main reconnect loop. Returns only when SIGTERM is
/// received (via `state.is_stopping()`).
pub async fn reconnect_loop(config: &ResolvedWorkerConfig, state: Arc<ShutdownState>) {
    let ceiling = config.backoff_ceiling_secs() as f64;
    let mut backoff = Backoff::new(ceiling);

    loop {
        if state.is_stopping() {
            tracing::info!("shutdown requested, exiting reconnect loop");
            return;
        }

        tracing::info!(url = %config.server_url, "connecting to server");

        match connect(config).await {
            Ok(stream) => {
                // Reset backoff on successful connection.
                backoff.reset();
                tracing::info!("connected to server");

                if let Err(err) = handler::run_connection(stream, config, Arc::clone(&state)).await
                {
                    tracing::warn!(%err, "connection closed");
                }
            }
            Err(err) => {
                tracing::warn!(%err, "connection attempt failed");
            }
        }

        if state.is_stopping() {
            tracing::info!("shutdown requested, exiting reconnect loop");
            return;
        }

        let delay = backoff.next_delay();
        tracing::info!(
            delay_secs = format!("{delay:.1}"),
            "reconnecting after backoff"
        );

        // Wait for either the backoff delay or a shutdown notification.
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_secs_f64(delay)) => {}
            () = state.notify.notified() => {
                tracing::info!("shutdown requested during backoff, exiting reconnect loop");
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Backoff
// ---------------------------------------------------------------------------

/// Exponential backoff with jitter and a configurable ceiling.
struct Backoff {
    initial_secs: f64,
    multiplier: f64,
    jitter_fraction: f64,
    ceiling_secs: f64,
    current: f64,
}

impl Backoff {
    fn new(ceiling_secs: f64) -> Self {
        Self {
            initial_secs: 1.0,
            multiplier: 2.0,
            jitter_fraction: 0.2,
            ceiling_secs,
            current: 0.0,
        }
    }

    /// Compute the next delay and advance the backoff state.
    fn next_delay(&mut self) -> f64 {
        if self.current == 0.0 {
            self.current = self.initial_secs;
        } else {
            self.current = (self.current * self.multiplier).min(self.ceiling_secs);
        }

        // Apply jitter: +-jitter_fraction of current delay.
        let mut rng = rand::thread_rng();
        let jitter = rng.gen_range(-self.jitter_fraction..=self.jitter_fraction);
        let delay = self.current * (1.0 + jitter);

        delay.max(0.1)
    }

    fn reset(&mut self) {
        self.current = 0.0;
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from a connection attempt.
#[derive(Debug)]
pub enum ConnectionError {
    Request(tungstenite::Error),
    InvalidHeader(String),
    WebSocket(tungstenite::Error),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(err) => write!(f, "invalid request: {err}"),
            Self::InvalidHeader(msg) => write!(f, "invalid header value: {msg}"),
            Self::WebSocket(err) => write!(f, "websocket error: {err}"),
        }
    }
}

impl std::error::Error for ConnectionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_starts_at_initial() {
        let mut b = Backoff::new(30.0);
        let d = b.next_delay();
        // Should be around 1.0 +/- 20% jitter.
        assert!(d > 0.5 && d < 1.5, "first delay {d} out of range");
    }

    #[test]
    fn backoff_doubles() {
        let mut b = Backoff::new(60.0);
        b.jitter_fraction = 0.0; // Disable jitter for deterministic test.
        let d1 = b.next_delay();
        assert!((d1 - 1.0).abs() < 0.01);
        let d2 = b.next_delay();
        assert!((d2 - 2.0).abs() < 0.01);
        let d3 = b.next_delay();
        assert!((d3 - 4.0).abs() < 0.01);
    }

    #[test]
    fn backoff_respects_ceiling() {
        let mut b = Backoff::new(5.0);
        b.jitter_fraction = 0.0;
        for _ in 0..20 {
            let d = b.next_delay();
            assert!(d <= 5.0, "delay {d} exceeds ceiling");
        }
    }

    #[test]
    fn backoff_reset() {
        let mut b = Backoff::new(30.0);
        b.jitter_fraction = 0.0;
        b.next_delay(); // 1
        b.next_delay(); // 2
        b.next_delay(); // 4
        b.reset();
        let d = b.next_delay();
        assert!(
            (d - 1.0).abs() < 0.01,
            "after reset, delay should be 1.0, got {d}"
        );
    }
}
