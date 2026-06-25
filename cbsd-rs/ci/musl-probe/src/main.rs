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

//! cbscore-rs C0 de-risk probe (design 001 "B1" / design 012).
//!
//! Constructs — but does not exercise against a live endpoint — an `aws-sdk-s3`
//! client in the exact shape C4/C5 use (custom endpoint + static credentials +
//! path-style addressing, invariant 9) over a ring-backed rustls connector, and
//! a `vaultrs` client, so the link includes both TLS/crypto stacks. There is no
//! live handshake, so the probe also runs in an EL container with no network.
//! CI asserts the resulting binary is fully static (musl) with no `openssl-sys`
//! and no `aws-lc-rs`.

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_smithy_http_client::{Builder, tls};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Explicit ring-backed rustls HTTPS connector: aws-sdk-s3's
    // `default-https-client` would link aws-lc-rs instead, so the connector is
    // built here with CryptoMode::Ring (design 012's musl-clean provider).
    let http_client = Builder::new()
        .tls_provider(tls::Provider::Rustls(
            tls::rustls_provider::CryptoMode::Ring,
        ))
        .build_https();

    let creds = Credentials::new("ak", "sk", None, None, "musl-probe");
    let s3_conf = aws_sdk_s3::Config::builder()
        .http_client(http_client)
        .region(Region::new("us-east-1"))
        .endpoint_url("https://example.invalid")
        .credentials_provider(creds)
        .force_path_style(true)
        .behavior_version(BehaviorVersion::latest())
        .build();
    let _s3 = aws_sdk_s3::Client::from_conf(s3_conf);

    let settings = vaultrs::client::VaultClientSettingsBuilder::default()
        .address("https://example.invalid:8200")
        .build()
        .expect("vault settings");
    let _vault = vaultrs::client::VaultClient::new(settings).expect("vault client");

    println!("musl-probe: constructed aws-sdk-s3 + vaultrs clients (rustls+ring)");
}
