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

//! Test-only helpers shared across the builder pipeline tests.

use crate::builder::BuilderError;

/// Retry a fallible async op a few times. The exec-spawning builder tests can
/// hit a transient `ETXTBSY` ("text file busy") when another test thread `fork`s
/// while this test's just-written script still has an open write fd — the
/// classic multithreaded write-then-exec race. It is a test-only artifact (real
/// build scripts come from the checked-out component repo, never written
/// concurrently), so a bounded retry is the right place to absorb it.
pub(crate) async fn retry_spawn<F, Fut, T>(mut op: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, BuilderError>>,
{
    let mut last = None;
    for _ in 0..25 {
        match op().await {
            Ok(value) => return value,
            Err(err) => {
                last = Some(err);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
    }
    panic!("operation kept failing: {}", last.unwrap());
}
