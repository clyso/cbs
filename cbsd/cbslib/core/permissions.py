# CBS service library - core - permissions
# Copyright (C) 2025  Clyso GmbH
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

import abc
import enum
import logging
import re
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Annotated, Literal, TypeVar

import pydantic
import yaml
from cbscore.errors import CESError

from cbslib.core import logger as parent_logger
from cbslib.logger import setup_logging

logger = parent_logger.getChild("permissions")
logger.setLevel(logging.INFO)


class PermissionsError(CESError):
    pass


class NotAuthorizedError(PermissionsError):
    """User is not authorized to perform an operation."""

    pass


class _PermissionsCapsMeta(abc.ABCMeta, enum.EnumMeta):
    pass


class _PermissionsCaps(enum.IntFlag, metaclass=_PermissionsCapsMeta):
    def get_canonical_name(self) -> str:
        if not self.name:
            return "unknown"
        return self.name.lower().replace("_", ":")

    @classmethod
    def get_cap(cls, name: str) -> _PermissionsCaps:
        if name == "all":
            return cls(~cls(0))
        cap_name = name.replace(":", "_").upper()
        try:
            return cls[cap_name]
        except KeyError:
            raise ValueError(f"unknown cap: {name}") from None


class AuthorizationCaps(_PermissionsCaps):
    BUILDS_CREATE = enum.auto()
    BUILDS_REVOKE_OWN = enum.auto()
    BUILDS_REVOKE_ANY = enum.auto()
    BUILDS_LIST_OWN = enum.auto()
    BUILDS_LIST_ANY = enum.auto()

    PROJECT_LIST = enum.auto()
    PROJECT_MANAGE = enum.auto()


class RoutesCaps(_PermissionsCaps):
    ROUTES_AUTH_PERMISSIONS = enum.auto()
    ROUTES_AUTH_LOGIN = enum.auto()

    ROUTES_BUILDS_NEW = enum.auto()
    ROUTES_BUILDS_REVOKE = enum.auto()
    ROUTES_BUILDS_STATUS = enum.auto()
    ROUTES_BUILDS_INSPECT = enum.auto()

    ROUTES_PERIODIC_BUILDS_NEW = enum.auto()
    ROUTES_PERIODIC_BUILDS_LIST = enum.auto()


def _caps_from_str_lst_for(
    t: type[_PermissionsCaps],
) -> Callable[[_PermissionsCaps | list[str]], _PermissionsCaps]:
    """
    Handle conversion from list of strings to PermissionsCaps.

    Receives a class that must extend _PermissionsCaps, and returns a function that
    will process the input from 'pydantic.BeforeValidator', converting a list of strings
    to the corresponding enum flags.
    """

    def _caps_from_str_lst(
        v: _PermissionsCaps | list[str],
    ) -> _PermissionsCaps:
        """Translate a list of text caps to their corresponding enum flags."""
        if isinstance(v, _PermissionsCaps):
            return v

        avail_caps = [cap.get_canonical_name() for cap in t if cap.name is not None]
        logger.debug(f"available caps for '{t.__name__}': {avail_caps}")

        def _inverted(values: list[str]) -> list[str]:
            """Invert all provided cap strings."""
            return [f"-{v}" for v in values]

        def _process(values: list[str]) -> list[str]:
            """
            Handle processing of the provided cap strings.

            Process the provided cap strings against the available caps,
            including handling of "all" and regex matching, expanding flags in bulk.

            Inverted/negated bulk flags will be expanded to their individual inverted
            flags.
            """
            logger.debug(f"processing cap strings: {values}")
            result: list[str] = []
            for v in values:
                flag_entry = rf"{v}"

                inverted = False
                if flag_entry.startswith("-"):
                    # flag is inverted. If bulk flag, all the expanded flags will also
                    # be inverted.
                    flag_entry = flag_entry[1:]
                    inverted = True

                if flag_entry == "all":
                    # map 'all' flag to a catch-all regex.
                    flag_entry = ".*"

                # expand all flags matching the provided regex
                value_results: list[str] = []
                for avail_cap in avail_caps:
                    try:
                        logger.debug(f"matching '{avail_cap}' against '{flag_entry}'")
                        if re.match(rf"{flag_entry}", avail_cap):
                            logger.debug(f"matched '{avail_cap}'")
                            value_results.append(avail_cap)
                    except Exception as e:
                        msg = (
                            rf"error matching '{avail_cap}' "
                            + f"against regex '{flag_entry}': {e}"
                        )
                        logger.error(msg)
                        raise ValueError(msg) from e

                # extend results with the resulting expanded flags, inverting them
                # if needed.
                result.extend(
                    value_results if not inverted else _inverted(value_results)
                )

            return result

        result = t(0)
        cap_strs = _process(v)

        # handle all processed and expanded caps strings, removing or adding as needed,
        # depending on whether these are inverted.
        for cap_str in cap_strs:
            if cap_str.startswith("-"):
                result &= t(~t.get_cap(cap_str[1:]))
            else:
                result |= t.get_cap(cap_str)

        logger.debug(
            f"converted cap strings '{v}' to flags '{result!r}', cap_strs: {cap_strs}"
        )
        return result

    return _caps_from_str_lst


def _caps_to_str_lst_for(
    t: type[_PermissionsCaps],
) -> Callable[[_PermissionsCaps], list[str]]:
    """
    Handle conversion from an enum extending _PermissionsCaps to a list of strings.

    Receives a class that must extend _PermissionsCaps, and returns a function that
    will convert the enum flags to a list of strings.
    """

    def _caps_to_str_lst(caps: _PermissionsCaps) -> list[str]:
        return [
            cap.get_canonical_name()
            for cap in t
            if cap in caps and cap.name is not None
        ]

    return _caps_to_str_lst


AuthCapsType = Annotated[
    AuthorizationCaps,
    pydantic.BeforeValidator(_caps_from_str_lst_for(AuthorizationCaps)),
    pydantic.PlainSerializer(_caps_to_str_lst_for(AuthorizationCaps)),
]

RoutesCapsType = Annotated[
    RoutesCaps,
    pydantic.BeforeValidator(_caps_from_str_lst_for(RoutesCaps)),
    pydantic.PlainSerializer(_caps_to_str_lst_for(RoutesCaps)),
]


class _AuthorizationEntry(pydantic.BaseModel):
    """Base class for authorization entries."""

    pass


class _PatternAuthorizationEntry(_AuthorizationEntry):
    """Keeps common fields for pattern-based authorization entries."""

    pattern: str

    def matches(self, what: str) -> bool:
        """Check whether the given string matches the pattern."""
        return re.match(self.pattern, what) is not None


class _CapsAuthorizationEntry[T](_AuthorizationEntry):
    """Keeps caps for authorization entries."""

    caps: T


class RegistryAuthorizationEntry(_PatternAuthorizationEntry):
    """Authorization entry for specific registries."""

    type: Literal["registry"] = "registry"


class RepositoryAuthorizationEntry(_PatternAuthorizationEntry):
    """Authorization entry for specific repositories."""

    type: Literal["repository"] = "repository"


class ProjectAuthorizationEntry(
    _PatternAuthorizationEntry, _CapsAuthorizationEntry[AuthCapsType]
):
    """Authorization entry for specific projects."""

    type: Literal["project"] = "project"


class RoutesAuthorizationEntry(_CapsAuthorizationEntry[RoutesCapsType]):
    """Authorization entry for specific routes."""

    type: Literal["routes"] = "routes"


AuthorizationEntry = Annotated[
    RegistryAuthorizationEntry
    | RepositoryAuthorizationEntry
    | ProjectAuthorizationEntry
    | RoutesAuthorizationEntry,
    pydantic.Field(discriminator="type"),
]


class AuthorizationGroup(pydantic.BaseModel):
    """Authorization group containing multiple authorization entries."""

    name: str
    authorized_for: list[AuthorizationEntry] = []


_T = TypeVar("_T", bound=enum.IntFlag)


class UserAuthorizationRule(pydantic.BaseModel):
    """Represents an authorization rule for users matching a certain pattern."""

    user_pattern: str
    groups: list[str] = []
    authorized_for: list[AuthorizationEntry] = []

    def matches(self, what: str) -> bool:
        """Check whether the given string matches the user pattern."""
        return re.match(self.user_pattern, what) is not None

    def _is_authorized_for_caps(
        self,
        authorized_for: list[AuthorizationEntry],
        entry_type: type[_CapsAuthorizationEntry[_T]],
        caps_type: type[_T],
        caps: _T,
        *,
        pattern: str | None = None,
    ) -> bool:
        """Check whether the provided entries authorize access for the given caps."""
        aggregated_caps: _T = caps_type(0)
        for entry in authorized_for:
            if not isinstance(entry, entry_type):
                continue

            if pattern and (
                not isinstance(entry, _PatternAuthorizationEntry)
                or not entry.matches(pattern)
            ):
                logger.debug(f"not authorized for pattern '{pattern}', entry '{entry}'")
                continue

            logger.debug(
                f"authorized for pattern '{pattern}', entry '{entry}', "
                + f"caps: '{entry.caps!r}'"
            )
            aggregated_caps |= entry.caps

        logger.debug(
            f"requested caps: '{caps!r}', aggregated caps: '{aggregated_caps!r}', "
            + f"intersection: '{caps & aggregated_caps!r}'"
        )
        return caps & aggregated_caps == caps

    def _is_authorized_for_caps_type(
        self,
        groups: dict[str, AuthorizationGroup],
        entry_type: type[_CapsAuthorizationEntry[_T]],
        caps_type: type[_T],
        caps: _T,
        *,
        pattern: str | None = None,
    ) -> bool:
        """Check whether this rule grants access for the given caps type."""
        if self._is_authorized_for_caps(
            self.authorized_for,
            entry_type,
            caps_type,
            caps,
            pattern=pattern,
        ):
            return True

        authorizations_for: list[AuthorizationEntry] = []
        for group_name in self.groups:
            if g := groups.get(group_name):
                authorizations_for.extend(g.authorized_for)

        logger.debug(f"check authorization from groups: {authorizations_for}")
        return self._is_authorized_for_caps(
            authorizations_for,
            entry_type,
            caps_type,
            caps,
            pattern=pattern,
        )

    def _is_authorized_for_pattern(
        self,
        authorized_for: list[AuthorizationEntry],
        entry_type: type[_PatternAuthorizationEntry],
        pattern: str,
    ) -> bool:
        """Check whether the provided entries authorize access for the given pattern."""
        for entry in authorized_for:
            if not isinstance(entry, entry_type):
                continue
            if entry.matches(pattern):
                return True
        return False

    def _is_authorized_pattern_type(
        self,
        groups: dict[str, AuthorizationGroup],
        pattern_type: type[_PatternAuthorizationEntry],
        pattern: str,
    ) -> bool:
        """Check whether this rule grants access for the given pattern type."""
        if self._is_authorized_for_pattern(self.authorized_for, pattern_type, pattern):
            return True

        authorizations_for: list[AuthorizationEntry] = []
        for group_name in self.groups:
            if g := groups.get(group_name):
                authorizations_for.extend(g.authorized_for)

        return self._is_authorized_for_pattern(
            authorizations_for, pattern_type, pattern
        )

    def is_registry_authorized(
        self,
        groups: dict[str, AuthorizationGroup],
        registry: str,
    ) -> bool:
        """Check whether this rule grants access for the given registry."""
        return self._is_authorized_pattern_type(
            groups, ProjectAuthorizationEntry, registry
        )

    def is_repository_authorized(
        self, groups: dict[str, AuthorizationGroup], repository: str
    ) -> bool:
        """Check whether this rule grants access for the given repository."""
        return self._is_authorized_pattern_type(
            groups, RepositoryAuthorizationEntry, repository
        )

    def is_project_authorized(
        self, groups: dict[str, AuthorizationGroup], project: str, caps: AuthCapsType
    ) -> bool:
        """Check whether this rule grants access for the given project's caps."""
        return self._is_authorized_for_caps_type(
            groups,
            ProjectAuthorizationEntry,
            AuthorizationCaps,
            caps,
            pattern=project,
        )

    def is_route_authorized(
        self, groups: dict[str, AuthorizationGroup], caps: RoutesCapsType
    ) -> bool:
        """Check whether this rule grants access for the given route's caps."""
        logger.warning(f"check authorization for route caps '{caps!r}'")
        return self._is_authorized_for_caps_type(
            groups,
            RoutesAuthorizationEntry,
            RoutesCaps,
            caps,
        )


class Permissions(pydantic.BaseModel):
    """Holds authorization groups and rules loaded from a permissions file."""

    groups: dict[str, AuthorizationGroup]
    rules: list[UserAuthorizationRule]

    @classmethod
    def load(cls, path: Path) -> Permissions:
        if not path or not path.exists() or not path.is_file():
            msg = f"authorizations file at '{path}' is not a file or does not exist"
            logger.error(msg)
            raise FileNotFoundError(msg)

        if path.suffix.lower() not in [".json", ".yaml", ".yml"]:
            msg = f"unsupported authorizations file type '{path.suffix}' at '{path}'"
            logger.error(msg)
            raise ValueError(msg)

        try:
            raw_data = path.read_text()
            return Permissions.model_validate(yaml.safe_load(raw_data))
        except (yaml.YAMLError, pydantic.ValidationError) as e:
            msg = f"error loading authorizations at '{path}':\n{e}"
            logger.error(msg)
            raise ValueError(msg) from e
        except Exception as e:
            msg = f"unexpected error loading authorizations at '{path}': {e}"
            logger.error(msg)
            raise CESError(msg) from e

    def is_authorized_for_project(
        self,
        user: str,
        project: str,
        caps: AuthCapsType,
    ) -> bool:
        """Check whether the given user is authorized for the given project."""
        for rule in self.rules:
            if not rule.matches(user):
                continue

            if rule.is_project_authorized(self.groups, project, caps):
                logger.debug(
                    f"user '{user}' authorized for project '{project}' "
                    + f"by rule '{rule.user_pattern}' with caps '{caps!r}'"
                )
                return True
            else:
                logger.warning(
                    f"user '{user}' not authorized for project '{project}', "
                    + f"rule '{rule}"
                )

        return False

    def is_authorized_for_registry(self, user: str, registry: str) -> bool:
        """Check whether the given user is authorized for the given registry."""
        for rule in self.rules:
            if not rule.matches(user):
                continue

            if rule.is_registry_authorized(self.groups, registry):
                logger.debug(
                    f"user '{user}' authorized for registry '{registry}' "
                    + f"by rule '{rule.user_pattern}'"
                )
                return True

        return False

    def is_authorized_for_repository(self, user: str, repository: str) -> bool:
        """Check whether the given user is authorized for the given repository."""
        for rule in self.rules:
            if not rule.matches(user):
                continue

            if rule.is_repository_authorized(self.groups, repository):
                logger.debug(
                    f"user '{user}' authorized for repository '{repository}' "
                    + f"by rule '{rule.user_pattern}'"
                )
                return True

        return False

    def is_authorized_for_route(self, user: str, caps: RoutesCaps) -> bool:
        """Check whether the given user is authorized for the given route's caps."""
        for rule in self.rules:
            if not rule.matches(user):
                continue

            if rule.is_route_authorized(self.groups, caps):
                logger.debug(
                    f"user '{user}' authorized for route caps '{caps!r}' "
                    + f"by rule '{rule.user_pattern}'"
                )
                return True

        return False

    def list_caps_for(
        self, user: str
    ) -> tuple[list[AuthorizationEntry], dict[str, list[AuthorizationEntry]]]:
        """Obtain a list of all authorization entries for the given user."""
        from_groups: dict[str, list[AuthorizationEntry]] = {}
        authorized_for: list[AuthorizationEntry] = []

        for rule in self.rules:
            if not rule.matches(user):
                continue

            authorized_for.extend(rule.authorized_for)

            for group in rule.groups:
                if group not in from_groups:
                    from_groups[group] = []

                if group not in self.groups:
                    logger.warning(f"unknown group '{group}' in rule for user '{user}'")
                    continue

                from_groups[group].extend(self.groups[group].authorized_for)

        return (authorized_for, from_groups)


# kludge for testing
#


def _test_basic_permissions() -> None:
    print("=> testing basic permissions model...")
    auth = Permissions(groups={}, rules=[])
    auth.groups["admin"] = AuthorizationGroup(
        name="admin",
        authorized_for=[
            ProjectAuthorizationEntry(
                pattern=r".*",
                caps=AuthorizationCaps.PROJECT_MANAGE
                | AuthorizationCaps.PROJECT_LIST
                | AuthorizationCaps.BUILDS_CREATE
                | AuthorizationCaps.BUILDS_REVOKE_ANY
                | AuthorizationCaps.BUILDS_LIST_ANY,
            ),
            RegistryAuthorizationEntry(pattern=r".*"),
            RepositoryAuthorizationEntry(pattern=r".*"),
        ],
    )
    auth.groups["development"] = AuthorizationGroup(
        name="development",
        authorized_for=[
            ProjectAuthorizationEntry(
                pattern=r"^dev/.*$",
                caps=AuthorizationCaps.PROJECT_LIST
                | AuthorizationCaps.BUILDS_CREATE
                | AuthorizationCaps.BUILDS_LIST_OWN
                | AuthorizationCaps.BUILDS_LIST_ANY
                | AuthorizationCaps.BUILDS_REVOKE_OWN,
            ),
            RegistryAuthorizationEntry(pattern=r"^registry\.example\.com/dev/.*$"),
            RepositoryAuthorizationEntry(pattern=r"https?://github\.com/clyso/.*"),
        ],
    )
    auth.groups["all"] = AuthorizationGroup(
        name="all",
        authorized_for=[
            ProjectAuthorizationEntry(
                pattern=r".*",
                caps=AuthorizationCaps.PROJECT_LIST
                | AuthorizationCaps.BUILDS_LIST_OWN
                | AuthorizationCaps.BUILDS_LIST_ANY,
            ),
        ],
    )
    auth.rules.extend(
        [
            UserAuthorizationRule(user_pattern=r"^.*@domain\.tld$", groups=["all"]),
            UserAuthorizationRule(
                user_pattern=r"^foo.bar@domain.tld$", groups=["development"]
            ),
            UserAuthorizationRule(
                user_pattern=r"^foo.baz@domain.tld$", groups=["admin"]
            ),
        ]
    )

    print(f"auth groups: {auth.groups}")
    print(f"auth rules: {auth.rules}")

    print("=> testing basic permissions checks...")
    assert auth.is_authorized_for_project(
        "foo.bar@domain.tld", "foo/bar", AuthorizationCaps.PROJECT_LIST
    )
    assert auth.is_authorized_for_project(
        "foo.bar@domain.tld", "dev/foobar", AuthorizationCaps.PROJECT_LIST
    )
    assert auth.is_authorized_for_project(
        "foo.bar@domain.tld",
        "dev/foobar",
        AuthorizationCaps.BUILDS_CREATE | AuthorizationCaps.BUILDS_LIST_ANY,
    )
    assert not auth.is_authorized_for_project(
        "foo.bar@domain.tld",
        "dev/foobar",
        AuthorizationCaps.PROJECT_MANAGE,
    )
    assert auth.is_authorized_for_project(
        "foo.baz@domain.tld",
        "dev/foobar",
        AuthorizationCaps.PROJECT_MANAGE
        | AuthorizationCaps.PROJECT_LIST
        | AuthorizationCaps.BUILDS_CREATE
        | AuthorizationCaps.BUILDS_REVOKE_ANY
        | AuthorizationCaps.BUILDS_LIST_ANY,
    )


def _test_default_permissions() -> None:
    default_yaml = r"""
groups:
  admin:
    name: admin
    authorized_for:
      - type: project
        pattern: '.*'
        caps:
          - '.*'
      - type: registry
        pattern: '.*'
      - type: repository
        pattern: '.*'
      - type: routes
        caps:
          - '.*'

  development:
    name: development
    authorized_for:
      - type: project
        pattern: '^dev/.*$'
        caps:
          - project:list
          - builds:create
          - builds:revoke:own
          - builds:list:own
          - builds:list:any
      - type: registry
        pattern: '^registry\.domain\.tld/dev/.*$'
      - type: repository
        pattern: '^https?://git\.domain\.tld/dev/.*$'
      - type: routes
        caps:
          - '.*'
          - -routes:auth:permissions

  all:
    name: all
    authorized_for:
      - type: project
        pattern: '.*'
        caps:
          - project:list
          - builds:list:any

rules:
  - user_pattern: '^.*@domain\.tld$'
    groups:
      - all
  - user_pattern: '^admin@domain\.tld$'
    groups:
      - admin
  - user_pattern: '^dev-.*@domain\.tld$'
    groups:
      - all
      - development
"""

    try:
        permissions = Permissions.model_validate(yaml.safe_load(default_yaml))
    except Exception as e:
        print(f"failed to load default permissions: {e}")
        sys.exit(1)

    assert permissions.is_authorized_for_project(
        "foo@domain.tld",
        "whatever/project",
        AuthorizationCaps.PROJECT_LIST | AuthorizationCaps.BUILDS_LIST_ANY,
    )
    assert not permissions.is_authorized_for_project(
        "foo@domain.tld", "whatever/project", AuthorizationCaps.BUILDS_CREATE
    )
    assert permissions.is_authorized_for_project(
        "admin@domain.tld",
        "other/project",
        AuthorizationCaps.PROJECT_MANAGE
        | AuthorizationCaps.BUILDS_CREATE
        | AuthorizationCaps.BUILDS_REVOKE_ANY
        | AuthorizationCaps.BUILDS_LIST_ANY,
    )
    assert permissions.is_authorized_for_project(
        "dev-bar@domain.tld",
        "dev/qwerty",
        AuthorizationCaps.PROJECT_LIST
        | AuthorizationCaps.BUILDS_CREATE
        | AuthorizationCaps.BUILDS_REVOKE_OWN
        | AuthorizationCaps.BUILDS_LIST_OWN,
    )
    assert not permissions.is_authorized_for_project(
        "dev-bar@domain.tld",
        "dev/qwerty",
        AuthorizationCaps.PROJECT_MANAGE | AuthorizationCaps.BUILDS_REVOKE_ANY,
    )
    assert permissions.is_authorized_for_route(
        "admin@domain.tld",
        caps=RoutesCaps.ROUTES_AUTH_PERMISSIONS | RoutesCaps.ROUTES_AUTH_LOGIN,
    )
    assert permissions.is_authorized_for_route(
        "dev-foo@domain.tld",
        caps=RoutesCaps.ROUTES_AUTH_LOGIN,
    )
    assert not permissions.is_authorized_for_route(
        "dev-foo@domain.tld",
        caps=RoutesCaps.ROUTES_AUTH_PERMISSIONS,
    )


if __name__ == "__main__":
    setup_logging()

    print("=> testing basic permissions...")
    _test_basic_permissions()

    print("=> testing default permissions...")
    _test_default_permissions()

    if len(sys.argv) > 1:
        perms_file = Path(sys.argv[1])
        print(f"=> loading permissions from '{perms_file}'...")
        try:
            permissions = Permissions.load(perms_file)
        except Exception as e:
            print(f"failed to load permissions: {e}")
            sys.exit(1)

        print(f"-> loaded permissions:\n{permissions.model_dump_json(indent=2)}")
        print(f"-> groups: {permissions.groups}")
        print(f"-> rules: {permissions.rules}")

    print("=> auth caps validation")
    caps = AuthorizationCaps.BUILDS_CREATE | AuthorizationCaps.BUILDS_LIST_OWN
    print(f"caps: {caps}")

    class Foo(pydantic.BaseModel):
        caps: AuthorizationCaps

    foo = Foo(caps=AuthorizationCaps.BUILDS_CREATE | AuthorizationCaps.BUILDS_LIST_OWN)
    print(f"original model:\n{foo}")
    json_caps = foo.model_dump_json(indent=2)
    print(f"serialized model:\n{json_caps}")
    new_foo = Foo.model_validate_json(json_caps)
    print(f"loaded model:\n{new_foo}")
    print(f"serialized new model:\n{new_foo.model_dump_json(indent=2)}")

    flags = AuthorizationCaps.BUILDS_CREATE
    print(f"contains builds:create: {flags in new_foo.caps}")
    flags = AuthorizationCaps.BUILDS_REVOKE_ANY
    print("contains builds:revoke:any: " + f"{flags in new_foo.caps}")
    flags = AuthorizationCaps.BUILDS_LIST_OWN
    print(f"contains builds:list:own: {flags & new_foo.caps}")
    flags = AuthorizationCaps.BUILDS_CREATE | AuthorizationCaps.BUILDS_LIST_OWN
    print("contains multiple: " + f"{flags in new_foo.caps}")
    flags = AuthorizationCaps.BUILDS_CREATE | AuthorizationCaps.BUILDS_REVOKE_ANY
    print(f"contains multiple, false: {flags in new_foo.caps}")
    res = AuthorizationCaps(0)
    for c in AuthorizationCaps:
        print(f"cap {c.name}: {c.value}")
        res |= c
    print(f"all flags: {res}")
    print(f"negated none: {~AuthorizationCaps(0)}")

    class Bar(pydantic.BaseModel):
        caps: AuthCapsType

    bar_raw_dict = {"caps": ["all"]}
    bar_all = Bar.model_validate(bar_raw_dict)
    print(f"with all flags: {bar_all}")

    bar_raw_dict = {"caps": [".*"]}
    bar_all_star = Bar.model_validate(bar_raw_dict)
    print(f"with all flags (*): {bar_all_star}")
    assert bar_all == bar_all_star

    bar_raw_dict = {"caps": ["all", "-builds:revoke:any"]}
    bar_some = Bar.model_validate(bar_raw_dict)
    print(f"with some flags: {bar_some}")

    bar_raw_dict = {"caps": ["builds:.*", "-builds:revoke:any"]}
    bar_some_builds = Bar.model_validate(bar_raw_dict)
    print(f"with some builds flags: {bar_some_builds}")

    bar_raw_dict = {"caps": ["all", "-builds:.*"]}
    bar_no_builds = Bar.model_validate(bar_raw_dict)
    print(f"with no builds flags: {bar_no_builds}")

    print("=> routes caps validation")

    class Baz(pydantic.BaseModel):
        caps: RoutesCapsType

    baz_raw_dict = {"caps": ["all"]}
    baz_all = Baz.model_validate(baz_raw_dict)
    print(f"with all flags: {baz_all}")

    baz_raw_dict = {"caps": [".*"]}
    baz_all_star = Baz.model_validate(baz_raw_dict)
    print(f"with all flags (*): {baz_all_star}")

    baz_raw_dict = {"caps": ["all", "-routes:auth:permissions"]}
    baz_some = Baz.model_validate(baz_raw_dict)
    print(f"with some flags: {baz_some}")

    baz_raw_dict = {"caps": ["routes:.*", "-routes:auth:permissions"]}
    baz_some_routes = Baz.model_validate(baz_raw_dict)
    print(f"with some routes flags: {baz_some_routes}")
