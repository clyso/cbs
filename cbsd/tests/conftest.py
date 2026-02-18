# CBS service daemon - tests - shared fixtures
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
from pathlib import Path
from typing import cast

import pytest
import yaml
from cbslib.config.config import Config
from cbslib.config.server import ServerConfig, ServerSecretsConfig
from cbslib.core.permissions import Permissions


def permissions_from_yaml(yaml_str: str) -> Permissions:
    """Build a Permissions object from an inline YAML string."""
    data = cast(object, yaml.safe_load(yaml_str))
    return Permissions.model_validate(data)


@pytest.fixture
def mock_config(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Config:
    """
    Provide a minimal Config with a test token secret key.

    Patches `cbslib.config.config._config` so that `get_config()` returns
    this config without needing real files or env vars.
    """
    import cbslib.config.config as config_mod

    cfg = Config(
        server=ServerConfig(
            cert=tmp_path / "cert.pem",
            key=tmp_path / "key.pem",
            db=tmp_path / "test.db",
            permissions=tmp_path / "permissions.yaml",
            secrets=ServerSecretsConfig(
                oauth2_secrets_file=str(tmp_path / "oauth2.json"),
                session_secret_key=secrets.token_hex(32),
                token_secret_key=secrets.token_hex(32),
                token_secret_ttl_minutes=60,
            ),
            logs=tmp_path / "logs",
        ),
        broker_url="redis://localhost:6379/0",
        results_backend_url="redis://localhost:6379/1",
        redis_backend_url="redis://localhost:6379/2",
    )
    monkeypatch.setattr(config_mod, "_config", cfg)
    return cfg
