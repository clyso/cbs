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

pub mod extractors;
pub mod oauth;
pub mod paseto;
pub mod token_cache;

/// Canonical form for an email used as a user identity key: trimmed and
/// lowercased.
///
/// Applied at every boundary where an externally-supplied email enters the
/// system (OAuth callback, the `seed_admin` config value, the admin entity
/// endpoints, user provisioning) so identity matching is case-insensitive and
/// every `users.email` is stored lowercase. See design 020.
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_email_trims_and_lowercases() {
        assert_eq!(normalize_email("  Alice@Example.COM "), "alice@example.com");
        assert_eq!(normalize_email("bob@x.com"), "bob@x.com");
        assert_eq!(normalize_email("ROBOT+CI@robots"), "robot+ci@robots");
    }
}
