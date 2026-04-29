# crt - store configuration
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.


from pathlib import Path

import pydantic
import yaml
from cbscore.versions.utils import parse_version

from crt.crtlib.errors.config import (
    AmbiguousChannelError,
    ChannelNotFoundError,
    ConfigError,
    ConfigNotFoundError,
)
from crt.crtlib.logger import logger as parent_logger

logger = parent_logger.getChild("config")

CONFIG_FILENAME = "crt.config.yaml"


class BrandingConfig(pydantic.BaseModel):
    product_name: str
    short_name: str
    docs_url: str | None = None
    vendor: str


class ChannelConfig(pydantic.BaseModel):
    description: str
    release_repo: str
    branding: BrandingConfig


class NamespaceConfig(pydantic.BaseModel):
    description: str
    channels: dict[str, ChannelConfig]


class CrtStoreConfig(pydantic.BaseModel):
    component: str
    namespaces: dict[str, NamespaceConfig]


ChannelResolution = tuple[str, str, ChannelConfig]


def load_config(repo_path: Path) -> CrtStoreConfig:
    """Load and validate the CRT store config from the repository root."""
    config_path = repo_path / CONFIG_FILENAME
    if not config_path.exists():
        raise ConfigNotFoundError(f"'{config_path}'")

    try:
        raw: object = yaml.safe_load(  # pyright: ignore[reportAny]
            config_path.read_text(encoding="utf-8")
        )
    except yaml.YAMLError as e:
        msg = f"malformed YAML in '{config_path}': {e}"
        logger.error(msg)
        raise ConfigError(msg) from None
    except Exception as e:
        msg = f"unable to read config at '{config_path}': {e}"
        logger.error(msg)
        raise ConfigError(msg) from None

    try:
        return CrtStoreConfig.model_validate(raw)
    except pydantic.ValidationError as e:
        msg = f"invalid config in '{config_path}': {e}"
        logger.error(msg)
        raise ConfigError(msg) from None


def resolve_channel(config: CrtStoreConfig, name: str) -> ChannelResolution:
    """
    Resolve a release/manifest name to its (namespace, channel, config).

    Extracts the channel prefix from the name (e.g., "ces" from
    "ces-v25.03.3") and looks it up across all namespaces. Channel prefixes
    must be globally unique within a store.
    """
    try:
        prefix, _, _, _, _ = parse_version(name)
    except ValueError as e:
        raise ChannelNotFoundError(f"cannot parse name '{name}': {e}") from None

    if not prefix:
        raise ChannelNotFoundError(f"no channel prefix in name '{name}'")

    matches: list[ChannelResolution] = []
    for ns_name, ns in config.namespaces.items():
        if prefix in ns.channels:
            matches.append((ns_name, prefix, ns.channels[prefix]))

    if len(matches) == 0:
        available = [ch for ns in config.namespaces.values() for ch in ns.channels]
        raise ChannelNotFoundError(
            f"prefix '{prefix}' not in config (available: {', '.join(available)})"
        )

    if len(matches) > 1:
        ns_names = ", ".join(m[0] for m in matches)
        raise AmbiguousChannelError(
            f"prefix '{prefix}' in multiple namespaces: {ns_names}"
        )

    return matches[0]


def get_release_repo(config: CrtStoreConfig, name: str) -> str:
    """Return the configured release repository for a given release name."""
    _, _, channel_config = resolve_channel(config, name)
    return channel_config.release_repo


def get_branding(config: CrtStoreConfig, name: str) -> BrandingConfig:
    """Return the branding config for a given release name."""
    _, _, channel_config = resolve_channel(config, name)
    return channel_config.branding
