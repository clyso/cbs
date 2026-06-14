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

import httpx
import pytest
from cbscore.__main__ import cmd_main
from click.testing import CliRunner

from integrationtests.cbscore import git, podman
from integrationtests.cbscore.conftest import GitData, ResourcePath


@pytest.mark.version("tc-v99.99.1")
@pytest.mark.container_name("tc")
@pytest.mark.container_tag("v99.99.1")
def test_cbscore_build(
    registry: str, git_local_repo: GitData, tmp_resources: ResourcePath
):
    # arrange
    git(git_local_repo.repo_path, "switch", "-c", "release/tc-v99.99.1")
    git(git_local_repo.repo_path, "tag", "-a", "tc-v99.99.1", "-m", '"test release"')
    git(
        git_local_repo.repo_path,
        "push",
        "--tags",
        "-u",
        "origin",
        "release/tc-v99.99.1",
    )

    # act
    runner = CliRunner()
    result = runner.invoke(
        cmd_main,
        [
            "-c",
            tmp_resources.config_path.absolute().as_posix(),
            "-d",
            "-l",
            "build",
            "--tls-verify=false",
            "--cbscore-path",
            tmp_resources.cbscore_path.absolute().as_posix(),
            "-e",
            tmp_resources.cbs_entrypoint_path.absolute().as_posix(),
            tmp_resources.desc_path.absolute().as_posix(),
        ],
    )

    # assert
    assert result.exit_code == 0
    url = f"http://{registry}/v2/tc/manifests/v99.99.1"
    auth = ("testuser", "testpass")
    headers = {
        "Accept": "application/vnd.oci.image.manifest.v1+json,"
        + "application/vnd.docker.distribution.manifest.v2+json"
    }
    response = httpx.get(url, auth=auth, headers=headers)
    assert response.status_code == 200

    _ = podman(
        "run", "--rm", "--tls-verify=false", f"{registry}/tc:v99.99.1", "test-component"
    )

    image_ids = podman("images", "--filter=reference=tc", "--format={{.Id}}")
    image_ids = [id.strip() for id in image_ids.splitlines() if id.strip()]
    if image_ids:
        _ = podman("rmi", *image_ids)
