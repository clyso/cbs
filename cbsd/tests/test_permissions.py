# CBS service daemon - tests - permissions
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

from pathlib import Path

import pydantic
import pytest
from cbslib.core.permissions import (
    AuthCapsType,
    AuthorizationCaps,
    AuthorizationGroup,
    Permissions,
    ProjectAuthorizationEntry,
    RegistryAuthorizationEntry,
    RepositoryAuthorizationEntry,
    RoutesCaps,
    RoutesCapsType,
    UserAuthorizationRule,
)

from tests.conftest import permissions_from_yaml

# ---------------------------------------------------------------------------
# Helpers for caps string validation via pydantic
# ---------------------------------------------------------------------------


class _AuthCapsModel(pydantic.BaseModel):
    caps: AuthCapsType


class _RoutesCapsModel(pydantic.BaseModel):
    caps: RoutesCapsType


def _auth_caps(strs: list[str]) -> AuthorizationCaps:
    return _AuthCapsModel.model_validate({"caps": strs}).caps


def _routes_caps(strs: list[str]) -> RoutesCaps:
    return _RoutesCapsModel.model_validate({"caps": strs}).caps


# ===========================================================================
# Caps string expansion
# ===========================================================================


class TestCapsStringExpansion:
    """Tests for the `_caps_from_str_lst_for` conversion logic."""

    def test_all_expands_to_every_auth_flag(self) -> None:
        caps = _auth_caps(["all"])
        for member in AuthorizationCaps:
            assert member in caps

    def test_star_regex_expands_same_as_all(self) -> None:
        assert _auth_caps([".*"]) == _auth_caps(["all"])

    def test_builds_star_expands_only_builds(self) -> None:
        caps = _auth_caps(["builds:.*"])
        for member in AuthorizationCaps:
            name = member.get_canonical_name()
            if name.startswith("builds:"):
                assert member in caps
            else:
                assert member not in caps

    def test_negation_removes_single_flag(self) -> None:
        caps = _auth_caps(["all", "-builds:revoke:any"])
        assert AuthorizationCaps.BUILDS_REVOKE_ANY not in caps
        assert AuthorizationCaps.BUILDS_CREATE in caps
        assert AuthorizationCaps.PROJECT_LIST in caps

    def test_all_minus_builds_star(self) -> None:
        caps = _auth_caps(["all", "-builds:.*"])
        for member in AuthorizationCaps:
            name = member.get_canonical_name()
            if name.startswith("builds:"):
                assert member not in caps
            else:
                assert member in caps

    def test_invalid_regex_raises_value_error(self) -> None:
        with pytest.raises(pydantic.ValidationError):
            _ = _auth_caps(["[invalid"])

    def test_routes_all_expands_to_every_route_flag(self) -> None:
        caps = _routes_caps(["all"])
        for member in RoutesCaps:
            assert member in caps

    def test_routes_star_same_as_all(self) -> None:
        assert _routes_caps([".*"]) == _routes_caps(["all"])

    def test_routes_negation(self) -> None:
        caps = _routes_caps(["all", "-routes:auth:permissions"])
        assert RoutesCaps.ROUTES_AUTH_PERMISSIONS not in caps
        assert RoutesCaps.ROUTES_AUTH_LOGIN in caps

    def test_routes_prefix_star_with_negation(self) -> None:
        caps = _routes_caps(["routes:.*", "-routes:auth:permissions"])
        assert RoutesCaps.ROUTES_AUTH_PERMISSIONS not in caps
        assert RoutesCaps.ROUTES_BUILDS_NEW in caps


# ===========================================================================
# Caps serialization roundtrip
# ===========================================================================


class TestCapsSerialization:
    """Caps -> JSON -> back roundtrip preserves the value."""

    def test_auth_caps_roundtrip(self) -> None:
        original = _AuthCapsModel(
            caps=AuthorizationCaps.BUILDS_CREATE | AuthorizationCaps.BUILDS_LIST_OWN,
        )
        json_str = original.model_dump_json()
        restored = _AuthCapsModel.model_validate_json(json_str)
        assert original.caps == restored.caps

    def test_routes_caps_roundtrip(self) -> None:
        original = _RoutesCapsModel(
            caps=RoutesCaps.ROUTES_BUILDS_NEW | RoutesCaps.ROUTES_AUTH_LOGIN,
        )
        json_str = original.model_dump_json()
        restored = _RoutesCapsModel.model_validate_json(json_str)
        assert original.caps == restored.caps

    def test_auth_caps_all_roundtrip(self) -> None:
        original = _AuthCapsModel.model_validate({"caps": ["all"]})
        json_str = original.model_dump_json()
        restored = _AuthCapsModel.model_validate_json(json_str)
        assert original.caps == restored.caps


# ===========================================================================
# Permissions.load()
# ===========================================================================

_MINIMAL_YAML = """\
groups: {}
rules: []
"""


class TestPermissionsLoad:
    """Tests for Permissions.load() from file."""

    def test_valid_yaml_loads(self, tmp_path: Path) -> None:
        f = tmp_path / "perms.yaml"
        _ = f.write_text(_MINIMAL_YAML)
        perms = Permissions.load(f)
        assert perms.groups == {}
        assert perms.rules == []

    def test_missing_file_raises_file_not_found(self, tmp_path: Path) -> None:
        with pytest.raises(FileNotFoundError):
            _ = Permissions.load(tmp_path / "nonexistent.yaml")

    def test_unsupported_extension_raises_value_error(self, tmp_path: Path) -> None:
        f = tmp_path / "perms.txt"
        _ = f.write_text(_MINIMAL_YAML)
        with pytest.raises(ValueError):
            _ = Permissions.load(f)

    def test_malformed_yaml_raises_value_error(self, tmp_path: Path) -> None:
        f = tmp_path / "perms.yaml"
        _ = f.write_text("groups: [invalid\n  yaml")
        with pytest.raises(ValueError):
            _ = Permissions.load(f)

    def test_invalid_schema_raises_value_error(self, tmp_path: Path) -> None:
        f = tmp_path / "perms.yaml"
        _ = f.write_text("groups: 42\nrules: 'nope'\n")
        with pytest.raises(ValueError):
            _ = Permissions.load(f)


# ===========================================================================
# Default YAML fixture — shared across permission check tests
# ===========================================================================

_DEFAULT_YAML = r"""
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


@pytest.fixture
def default_perms() -> Permissions:
    return permissions_from_yaml(_DEFAULT_YAML)


# ===========================================================================
# is_authorized_for_project
# ===========================================================================


class TestIsAuthorizedForProject:
    """Tests for Permissions.is_authorized_for_project()."""

    def test_catch_all_rule_grants_limited_caps(
        self, default_perms: Permissions
    ) -> None:
        assert default_perms.is_authorized_for_project(
            "foo@domain.tld",
            "whatever/project",
            AuthorizationCaps.PROJECT_LIST | AuthorizationCaps.BUILDS_LIST_ANY,
        )

    def test_catch_all_rule_denies_caps_not_granted(
        self, default_perms: Permissions
    ) -> None:
        assert not default_perms.is_authorized_for_project(
            "foo@domain.tld",
            "whatever/project",
            AuthorizationCaps.BUILDS_CREATE,
        )

    def test_admin_gets_all_project_caps(self, default_perms: Permissions) -> None:
        assert default_perms.is_authorized_for_project(
            "admin@domain.tld",
            "other/project",
            AuthorizationCaps.PROJECT_MANAGE
            | AuthorizationCaps.BUILDS_CREATE
            | AuthorizationCaps.BUILDS_REVOKE_ANY
            | AuthorizationCaps.BUILDS_LIST_ANY,
        )

    def test_dev_user_authorized_for_dev_project(
        self, default_perms: Permissions
    ) -> None:
        assert default_perms.is_authorized_for_project(
            "dev-bar@domain.tld",
            "dev/qwerty",
            AuthorizationCaps.PROJECT_LIST
            | AuthorizationCaps.BUILDS_CREATE
            | AuthorizationCaps.BUILDS_REVOKE_OWN
            | AuthorizationCaps.BUILDS_LIST_OWN,
        )

    def test_dev_user_denied_admin_caps_on_dev_project(
        self, default_perms: Permissions
    ) -> None:
        assert not default_perms.is_authorized_for_project(
            "dev-bar@domain.tld",
            "dev/qwerty",
            AuthorizationCaps.PROJECT_MANAGE | AuthorizationCaps.BUILDS_REVOKE_ANY,
        )

    def test_unknown_user_denied(self, default_perms: Permissions) -> None:
        assert not default_perms.is_authorized_for_project(
            "nobody@other.tld",
            "any/project",
            AuthorizationCaps.PROJECT_LIST,
        )


# ===========================================================================
# Multi-rule evaluation — core of permissions bug investigation
# ===========================================================================


class TestMultiRuleEvaluation:
    """
    Verify that multiple matching rules are all evaluated.

    This tests the scenario where a user matches a catch-all rule with limited
    caps AND a more specific rule with full caps. The second rule must still
    grant access even though the first rule's limited caps didn't.
    """

    def test_admin_matches_catch_all_then_admin_rule(
        self, default_perms: Permissions
    ) -> None:
        """
        admin@domain.tld matches the catch-all (limited caps) AND admin rule.

        The admin rule must grant PROJECT_MANAGE even though the catch-all doesn't.
        """
        assert default_perms.is_authorized_for_project(
            "admin@domain.tld",
            "any/project",
            AuthorizationCaps.PROJECT_MANAGE,
        )

    def test_dev_user_gets_caps_from_both_all_and_dev_groups(
        self, default_perms: Permissions
    ) -> None:
        """
        dev-x@domain.tld has both 'all' and 'development' groups.

        Caps from both groups should be aggregated via OR within a single rule.
        """
        assert default_perms.is_authorized_for_project(
            "dev-x@domain.tld",
            "dev/test",
            AuthorizationCaps.BUILDS_CREATE | AuthorizationCaps.BUILDS_LIST_ANY,
        )

    def test_catch_all_alone_does_not_grant_admin_caps(
        self, default_perms: Permissions
    ) -> None:
        """A user matching only the catch-all rule should not get admin caps."""
        assert not default_perms.is_authorized_for_project(
            "regular@domain.tld",
            "some/project",
            AuthorizationCaps.PROJECT_MANAGE,
        )


# ===========================================================================
# is_authorized_for_route
# ===========================================================================


class TestIsAuthorizedForRoute:
    """Tests for Permissions.is_authorized_for_route()."""

    def test_admin_gets_all_route_caps(self, default_perms: Permissions) -> None:
        assert default_perms.is_authorized_for_route(
            "admin@domain.tld",
            caps=RoutesCaps.ROUTES_AUTH_PERMISSIONS | RoutesCaps.ROUTES_AUTH_LOGIN,
        )

    def test_dev_gets_routes_except_excluded(self, default_perms: Permissions) -> None:
        assert default_perms.is_authorized_for_route(
            "dev-foo@domain.tld",
            caps=RoutesCaps.ROUTES_AUTH_LOGIN,
        )

    def test_dev_denied_excluded_route(self, default_perms: Permissions) -> None:
        assert not default_perms.is_authorized_for_route(
            "dev-foo@domain.tld",
            caps=RoutesCaps.ROUTES_AUTH_PERMISSIONS,
        )

    def test_unknown_user_denied(self, default_perms: Permissions) -> None:
        assert not default_perms.is_authorized_for_route(
            "nobody@other.tld",
            caps=RoutesCaps.ROUTES_AUTH_LOGIN,
        )


# ===========================================================================
# is_authorized_for_registry
# ===========================================================================


class TestIsAuthorizedForRegistry:
    """Tests for Permissions.is_authorized_for_registry()."""

    def test_admin_authorized_for_any_registry(
        self, default_perms: Permissions
    ) -> None:
        assert default_perms.is_authorized_for_registry(
            "admin@domain.tld", "any.registry.io/foo"
        )

    def test_dev_authorized_for_dev_registry(self, default_perms: Permissions) -> None:
        assert default_perms.is_authorized_for_registry(
            "dev-foo@domain.tld", "registry.domain.tld/dev/myimg"
        )

    def test_dev_denied_non_dev_registry(self, default_perms: Permissions) -> None:
        assert not default_perms.is_authorized_for_registry(
            "dev-foo@domain.tld", "registry.domain.tld/prod/myimg"
        )

    def test_regular_user_denied_registry(self, default_perms: Permissions) -> None:
        """The 'all' group has no registry entries, so regular users are denied."""
        assert not default_perms.is_authorized_for_registry(
            "regular@domain.tld", "any.registry.io/foo"
        )


# ===========================================================================
# is_authorized_for_repository
# ===========================================================================


class TestIsAuthorizedForRepository:
    """Tests for Permissions.is_authorized_for_repository()."""

    def test_admin_authorized_for_any_repo(self, default_perms: Permissions) -> None:
        assert default_perms.is_authorized_for_repository(
            "admin@domain.tld", "https://github.com/anything"
        )

    def test_dev_authorized_for_dev_repo(self, default_perms: Permissions) -> None:
        assert default_perms.is_authorized_for_repository(
            "dev-foo@domain.tld", "https://git.domain.tld/dev/myrepo"
        )

    def test_dev_denied_non_dev_repo(self, default_perms: Permissions) -> None:
        assert not default_perms.is_authorized_for_repository(
            "dev-foo@domain.tld", "https://git.domain.tld/prod/myrepo"
        )

    def test_regular_user_denied_repository(self, default_perms: Permissions) -> None:
        assert not default_perms.is_authorized_for_repository(
            "regular@domain.tld", "https://github.com/anything"
        )


# ===========================================================================
# Basic Permissions object construction (ported from _test_basic_permissions)
# ===========================================================================


class TestBasicPermissionsConstruction:
    """Tests built from the ad-hoc _test_basic_permissions() assertions."""

    @pytest.fixture
    def basic_perms(self) -> Permissions:
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
                    user_pattern=r"^foo.bar@domain.tld$",
                    groups=["development"],
                ),
                UserAuthorizationRule(
                    user_pattern=r"^foo.baz@domain.tld$",
                    groups=["admin"],
                ),
            ]
        )
        return auth

    def test_dev_user_project_list_any(self, basic_perms: Permissions) -> None:
        assert basic_perms.is_authorized_for_project(
            "foo.bar@domain.tld", "foo/bar", AuthorizationCaps.PROJECT_LIST
        )

    def test_dev_user_dev_project_list(self, basic_perms: Permissions) -> None:
        assert basic_perms.is_authorized_for_project(
            "foo.bar@domain.tld",
            "dev/foobar",
            AuthorizationCaps.PROJECT_LIST,
        )

    def test_dev_user_combined_caps(self, basic_perms: Permissions) -> None:
        assert basic_perms.is_authorized_for_project(
            "foo.bar@domain.tld",
            "dev/foobar",
            AuthorizationCaps.BUILDS_CREATE | AuthorizationCaps.BUILDS_LIST_ANY,
        )

    def test_dev_user_denied_manage(self, basic_perms: Permissions) -> None:
        assert not basic_perms.is_authorized_for_project(
            "foo.bar@domain.tld",
            "dev/foobar",
            AuthorizationCaps.PROJECT_MANAGE,
        )

    def test_admin_user_full_caps(self, basic_perms: Permissions) -> None:
        assert basic_perms.is_authorized_for_project(
            "foo.baz@domain.tld",
            "dev/foobar",
            AuthorizationCaps.PROJECT_MANAGE
            | AuthorizationCaps.PROJECT_LIST
            | AuthorizationCaps.BUILDS_CREATE
            | AuthorizationCaps.BUILDS_REVOKE_ANY
            | AuthorizationCaps.BUILDS_LIST_ANY,
        )
