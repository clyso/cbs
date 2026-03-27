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

import os
import shutil
import uuid
from collections.abc import Generator
from pathlib import Path
from string import Template

import httpx
import pytest
from _pytest.fixtures import SubRequest
from testcontainers.core.container import (  # pyright: ignore[reportMissingTypeStubs]
    DockerContainer,
)
from testcontainers.core.wait_strategies import (  # pyright: ignore[reportMissingTypeStubs]
    HttpWaitStrategy,
)

from tests.integration.cbscore import git, podman

_username = "testuser"
_password = "testpass"  # noqa: S105 it is intended to be hardcoded
_hashed_pass = "$apr1$PG4UgaB4$967wqWKtFcAkT/EG/SjcP1"  # noqa: S105 it is intended to be hardcoded


@pytest.fixture(scope="session", autouse=True)
def configure_test_env() -> None:
    """Configure the environment for podman and Testcontainers."""
    os.environ["TESTCONTAINERS_RYUK_DISABLED"] = "true"

    if "DOCKER_HOST" not in os.environ:
        uid = os.getuid()
        podman_sock = f"unix:///run/user/{uid}/podman/podman.sock"
        os.environ["DOCKER_HOST"] = podman_sock

    os.environ["CBS_DEV"] = "true"


@pytest.fixture(scope="session")
def registry() -> Generator[str]:
    """Start Docker Registry with Basic Auth."""
    # Pre-generated htpasswd for testuser:testpass
    htpasswd_content = f"{_username}:{_hashed_pass}"

    registry = DockerContainer("registry:2").with_exposed_ports(5000)

    with registry:
        wrapped = registry.get_wrapped_container()
        _ = wrapped.exec_run("mkdir /auth")
        _ = wrapped.exec_run(f"sh -c 'echo {htpasswd_content} > /auth/htpasswd'")

        host_ip: str = registry.get_container_host_ip()
        host_port: int = int(registry.get_exposed_port(5000))

        url: str = f"{host_ip}:{host_port}"
        yield url


@pytest.fixture(scope="session")
def gitea() -> Generator[str]:
    """Start a Gitea container and seeds it with a repository."""
    image_name = "docker.io/gitea/gitea:1.21-rootless"
    _ = podman("pull", image_name)

    gitea = (
        DockerContainer(image_name)
        .with_env("USER_UID", "1000")
        .with_env("GITEA__security__INSTALL_LOCK", "true")  # Skip setup
        .with_exposed_ports(3000)
        .waiting_for(
            HttpWaitStrategy.from_url(
                "http://0.0.0.0:3000/api/v1/version"
            ).for_status_code(200)
        )
    )

    with gitea:
        wrapped = gitea.get_wrapped_container()
        _ = wrapped.exec_run(
            f"gitea admin user create --username {_username} --password {_password} "
            + "--email testuser@testcorp.com --admin --must-change-password=false"
        )

        host_ip = gitea.get_container_host_ip()
        port = gitea.get_exposed_port(3000)

        yield f"{host_ip}:{port}"


class GitData:
    repo_path: Path
    repo_name: str
    repo_url: str

    def __init__(self, repo_path: Path, repo_name: str, repo_url: str):
        self.repo_path = repo_path
        self.repo_name = repo_name
        self.repo_url = repo_url


@pytest.fixture
def git_local_repo(tmp_path: Path, gitea: str) -> Generator[GitData]:
    """Create a temporary local Git repository initialized with test data."""
    repo_path: Path = tmp_path / "test-component-repo"
    repo_path.mkdir()

    resource_src: Path = (
        Path(__file__).parents[2] / "resources" / "cbscore" / "test-component"
    )

    _ = shutil.copytree(resource_src, repo_path, dirs_exist_ok=True)

    repo_name = f"test-repo-{uuid.uuid4()}"

    _ = httpx.post(
        f"http://{gitea}/api/v1/user/repos",
        auth=(_username, _password),
        json={"name": repo_name, "private": False},
    )
    remote_url = f"http://{_username}:{_password}@{gitea}/{_username}/{repo_name}.git"

    git(repo_path, "init", "-b", "main")
    git(repo_path, "config", "user.email", "testuser@testcorp.com")
    git(repo_path, "config", "user.name", _username)
    git(repo_path, "config", "commit.gpgsign", "false")
    git(repo_path, "remote", "add", "origin", remote_url)
    git(repo_path, "add", ".")
    git(repo_path, "commit", "-m", "Initial commit for integration test")
    git(repo_path, "push", "-u", "origin", "main")

    yield GitData(repo_path, repo_name, f"http://{gitea}/{_username}/{repo_name}.git")

    _ = httpx.delete(
        f"http://{gitea}/api/v1/repos/{_username}/{repo_name}",
        auth=(_username, _password),
    )


class ResourcePath:
    config_path: Path
    desc_path: Path
    cbscore_path: Path
    cbs_entrypoint_path: Path

    def __init__(
        self,
        config_path: Path,
        desc_file: Path,
        cbscore_path: Path,
        cbs_entrypoint_path: Path,
    ):
        self.config_path = config_path
        self.desc_path = desc_file
        self.cbscore_path = cbscore_path
        self.cbs_entrypoint_path = cbs_entrypoint_path


@pytest.fixture
def tmp_resources(
    request: SubRequest,
    tmp_path: Path,
    registry: str,
    gitea: str,
    git_local_repo: GitData,
) -> ResourcePath:
    version = _get_value_or(request, "version", default="tc-v99.99.1")
    container_name = _get_value_or(request, "container_name", "tc")
    container_tag = _get_value_or(request, "container_tag", "v99.99.1")

    resources = tmp_path / "resources"
    resources.mkdir(parents=True, exist_ok=True)

    scratch = resources / "scratch"
    scratch.mkdir(parents=True, exist_ok=True)

    scratch_container = resources / "scratch-container"
    scratch_container.mkdir(parents=True, exist_ok=True)

    cbs = Path(__file__).parents[3]

    cbscore_resources = cbs / "tests" / "resources" / "cbscore"

    components = resources / "components"
    _ = shutil.copytree(
        cbscore_resources / "components", components, dirs_exist_ok=True
    )

    _ = _copy_file(
        components / "test-component" / "cbs.component.yaml",
        repo=git_local_repo.repo_url,
    )

    secrets_file = _copy_file(
        cbscore_resources / "secrets.yaml",
        resources,
        reg_key=registry.split(":")[0],
        reg_address=registry,
        gitea_url=gitea,
    )

    desc_file = _copy_file(
        cbscore_resources / "version_desc.json",
        resources,
        registry=registry,
        git_repo=git_local_repo.repo_url,
        version=version,
        container_name=container_name,
        container_tag=container_tag,
    )

    config_file = _copy_file(
        cbscore_resources / "cbs-build.config.yaml",
        resources,
        components=components.absolute().as_posix(),
        scratch=scratch.absolute().as_posix(),
        scratch_containers=scratch_container.absolute().as_posix(),
        secrets=str(secrets_file),
    )

    cbscore_path = cbs / "cbscore"

    cbs_entrypoint_path = (
        cbscore_path / "src" / "cbscore" / "_tools" / "cbscore-entrypoint.sh"
    )

    return ResourcePath(config_file, desc_file, cbscore_path, cbs_entrypoint_path)


@pytest.fixture(scope="session", autouse=True)
def podman_session_cleanup():
    yield
    _ = podman("system", "prune", "--force", "--volumes")


def _copy_file(file: Path, dst: Path | None = None, **kwargs: str) -> Path:
    txt = Template(file.read_text())
    txt = txt.safe_substitute(**kwargs)
    if not dst:
        dst = file.parent
    name = file.name
    ret = dst / name

    _ = ret.write_text(txt)
    return ret


def _get_value_or[T](request: SubRequest, key: str, default: T) -> T:
    marker = request.node.get_closest_marker(key)
    return marker.args[0] if marker else default
