# CBS service daemon - tests - auth tokens
# Copyright (C) 2026  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.

from __future__ import annotations

import secrets

import pyseto
import pytest
from cbslib.auth.auth import UnauthorizedTokenError, token_create, token_decode
from cbslib.config.config import Config

# ===========================================================================
# Token roundtrip
# ===========================================================================


class TestTokenRoundtrip:
    """Token create -> decode roundtrip."""

    def test_create_then_decode_preserves_email(self, mock_config: Config) -> None:
        _ = mock_config  # fixture used for side-effects (patches global config)
        token = token_create("user@example.com")
        decoded = token_decode(token.token.get_secret_value().decode())
        assert decoded.user == "user@example.com"

    def test_expiration_set_when_ttl_configured(self, mock_config: Config) -> None:
        _ = mock_config  # fixture used for side-effects (patches global config)
        token = token_create("user@example.com")
        assert token.info.expires is not None

    def test_expiration_none_when_ttl_zero(
        self, mock_config: Config, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        assert mock_config.server is not None
        monkeypatch.setattr(mock_config.server.secrets, "token_secret_ttl_minutes", 0)
        token = token_create("user@example.com")
        assert token.info.expires is None


# ===========================================================================
# Invalid tokens
# ===========================================================================


@pytest.mark.usefixtures("mock_config")
class TestInvalidTokens:
    """Decoding invalid tokens must raise UnauthorizedTokenError."""

    def test_garbage_string_raises(self) -> None:
        with pytest.raises(UnauthorizedTokenError):
            _ = token_decode("totally-not-a-valid-token")

    def test_different_key_raises(self) -> None:
        """A token signed with a different key cannot be decoded."""
        different_key = pyseto.Key.new(
            version=4,
            purpose="local",
            key=secrets.token_hex(32),
        )
        foreign_token: bytes = pyseto.encode(
            different_key,
            payload=b'{"user":"evil@example.com","expires":null}',
        )
        with pytest.raises(UnauthorizedTokenError):
            _ = token_decode(foreign_token.decode())

    def test_empty_string_raises(self) -> None:
        with pytest.raises(UnauthorizedTokenError):
            _ = token_decode("")


# ===========================================================================
# Malformed payload
# ===========================================================================


class TestMalformedPayload:
    """Valid PASETO envelope but non-JSON payload."""

    def test_non_json_payload_raises(self, mock_config: Config) -> None:
        assert mock_config.server is not None
        key = pyseto.Key.new(
            version=4,
            purpose="local",
            key=mock_config.server.secrets.token_secret_key,
        )
        bad_token: bytes = pyseto.encode(
            key,
            payload=b"this is not json",
        )
        with pytest.raises(UnauthorizedTokenError):
            _ = token_decode(bad_token.decode())
