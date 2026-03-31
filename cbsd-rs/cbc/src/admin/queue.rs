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

//! Build queue status command.

use serde::Deserialize;

use crate::client::CbcClient;
use crate::config::Config;
use crate::error::Error;

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct QueueStatus {
    #[serde(default)]
    high: u64,
    #[serde(default)]
    normal: u64,
    #[serde(default)]
    low: u64,
}

// ---------------------------------------------------------------------------
// admin queue
// ---------------------------------------------------------------------------

pub async fn run(
    config_path: Option<&std::path::Path>,
    debug: bool,
    no_tls_verify: bool,
) -> Result<(), Error> {
    let config = Config::load(config_path)?;
    let client = CbcClient::new(&config.host, &config.token, debug, no_tls_verify)?;

    let status: QueueStatus = client.get("admin/queue").await?;

    println!("  {:<10} PENDING", "PRIORITY");
    println!("  {:<10} {}", "high", status.high);
    println!("  {:<10} {}", "normal", status.normal);
    println!("  {:<10} {}", "low", status.low);

    Ok(())
}
