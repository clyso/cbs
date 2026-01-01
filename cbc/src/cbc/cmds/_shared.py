# cbc - commands - shared functionality
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

import errno
import functools
import logging
import sys
from collections.abc import Callable
from typing import Any, ParamSpec, TypeVar

import click
from cbscore.versions.errors import VersionError
from cbscore.versions.utils import VersionType, get_version_type, parse_component_refs
from cbsdcore.auth.user import UserConfig
from cbsdcore.versions import (
    BuildArch,
    BuildArtifactType,
    BuildComponent,
    BuildDescriptor,
    BuildDestImage,
    BuildSignedOffBy,
    BuildTarget,
)
from click.decorators import FC as C_FC

from cbc.auth import auth_whoami
from cbc.client import CBCConnectionError, CBCPermissionDeniedError

_P = ParamSpec("_P")
_R = TypeVar("_R")


def build_descriptor_options(
    func: Callable[..., Any],  # pyright: ignore[reportExplicitAny]
) -> Callable[_P, C_FC]:
    """Define shared options for creating a build descriptor."""

    @click.option(
        "-t",
        "--type",
        "version_type_name",
        type=click.Choice([t.value for t in VersionType], case_sensitive=True),
        help="Type of version to be built",
        required=False,
        metavar="TYPE",
        default="dev",
        show_default=True,
    )
    @click.option(
        "-p",
        "--channel",
        "version_channel",
        type=str,
        help="Channel to build version for",
        required=True,
        metavar="CHANNEL",
        default="!user",
        show_default=True,
    )
    @click.option(
        "-c",
        "--component",
        "components",
        type=str,
        multiple=True,
        required=True,
        metavar="NAME@VERSION",
        help="Component's version (e.g., 'ceph@abcde1234')",
    )
    @click.option(
        "--override-component",
        "component_overrides",
        type=str,
        multiple=True,
        required=False,
        metavar="COMPONENT=URL",
        help="Override component's location",
    )
    @click.option(
        "--distro",
        type=str,
        required=False,
        default="rockylinux:9",
        metavar="NAME",
        help="Distribution to use for this release",
    )
    @click.option(
        "--el-version",
        type=int,
        required=False,
        default=9,
        metavar="VERSION",
        help="Distribution's EL version",
    )
    # @click.option(
    #     "--registry",
    #     type=str,
    #     required=False,
    #     default="harbor.clyso.com",
    #     metavar="URL",
    #     help="Registry for this release's image",
    # )
    @click.option(
        "--image-name",
        type=str,
        required=False,
        default="ceph/ceph",
        metavar="NAME",
        help="Name for this release's image",
    )
    @click.option(
        "--image-tag",
        type=str,
        required=False,
        metavar="TAG",
        help="Tag for this release's image",
    )
    def inner(*args: _P.args, **kwargs: _P.kwargs) -> C_FC:
        return func(*args, **kwargs)  # pyright: ignore[reportAny]

    return functools.update_wrapper(inner, func)


def new_build_descriptor_helper(
    config: UserConfig,
    logger: logging.Logger,
    *,
    version: str,
    version_type_name: str,
    version_channel: str,
    components: tuple[str, ...],
    component_overrides: tuple[str, ...],
    distro: str,
    el_version: int,
    # _registry: str | None = None,  # currently unused?
    image_name: str,
    image_tag: str | None,
) -> BuildDescriptor:
    """Create a new build descriptor from provided arguments."""
    try:
        email, name = auth_whoami(logger, config)
    except CBCConnectionError as e:
        click.echo(f"connection error: {e}", err=True)
        sys.exit(errno.ECONNREFUSED)
    except CBCPermissionDeniedError as e:
        click.echo(f"permission denied: {e}", err=True)
        sys.exit(errno.EACCES)
    except Exception as e:
        click.echo(f"error obtaining user's info: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        version_type = get_version_type(version_type_name)
    except VersionError as e:
        click.echo(f"error parsing version type: {e}", err=True)
        sys.exit(errno.EINVAL)

    try:
        component_refs = parse_component_refs(list(components))
    except VersionError as e:
        click.echo(f"error parsing components: {e}", err=True)
        sys.exit(errno.EINVAL)

    uri_overrides: dict[str, str] = {}
    for uri_override in component_overrides:
        entries = uri_override.split("=", maxsplit=1)
        if len(entries) != 2:
            click.echo(f"malformed component URI override: '{uri_override}'", err=True)
            sys.exit(errno.EINVAL)

        comp, uri = entries
        if comp not in component_refs:
            click.echo(f"ignoring URI for missing component '{comp}'", err=True)
            continue

        uri_overrides[comp] = uri

    components_lst: list[BuildComponent] = []
    for comp_name, comp_ref in component_refs.items():
        components_lst.append(
            BuildComponent(
                name=comp_name,
                ref=comp_ref,
                repo=uri_overrides.get(comp_name),
            )
        )

    image_tag = image_tag or version

    return BuildDescriptor(
        version=version,
        channel=version_channel,
        signed_off_by=BuildSignedOffBy(user=name, email=email),
        version_type=version_type,
        dst_image=BuildDestImage(name=image_name, tag=image_tag),
        components=components_lst,
        build=BuildTarget(
            distro=distro,
            os_version=f"el{el_version}",
            artifact_type=BuildArtifactType.rpm,
            arch=BuildArch.x86_64,
        ),
    )
