# CES library - buildah utilities
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


from datetime import datetime as dt
from datetime import timezone as tz
import os
from pathlib import Path
import tempfile
from typing import Callable, override

from ceslib.errors import CESError
from ceslib.images.signing import SigningError, async_sign
from ceslib.utils import CommandError, async_run_cmd
from ceslib.utils import log as parent_logger
from ceslib.utils.secrets import SecretsVaultError, SecretsVaultMgr
from ceslib.versions.desc import VersionDescriptor

log = parent_logger.getChild("buildah")


class BuildahError(CESError):
    @override
    def __str__(self) -> str:
        return f"buildah error: {self.msg}"


async def _buildah_run(
    cmd: list[str],
    *,
    cid: str | None = None,
    args: list[str] | None = None,
    with_args_divider: bool = False,
    outcb: Callable[[str], None] | None = None,
) -> tuple[int, str, str]:
    if len(cmd) == 0:
        msg = "no commands provided to buildah"
        log.error(msg)
        raise BuildahError(msg)

    cmd = ["buildah"] + cmd

    if cid:
        cmd.append(cid)

    if args:
        if with_args_divider:
            cmd.append("--")
        cmd.extend(args)

    try:
        log.debug(f"run buildah command: {cmd}")
        rc, stdout, stderr = await async_run_cmd(cmd, outcb=outcb)
    except CommandError as e:
        msg = f"error running buildah: {e}"
        log.error(msg)
        raise BuildahError(msg)
    except Exception as e:
        msg = f"unknown error running buildah: {e}"
        log.error(msg)
        raise BuildahError(msg)

    if rc != 0:
        log.error(f"error running buildah ({rc}): {stderr}")

    return rc, stdout, stderr


class BuildahContainer:
    cid: str
    version_desc: VersionDescriptor
    is_committed: bool

    def __init__(self, cid: str, desc: VersionDescriptor) -> None:
        self.cid = cid
        self.version_desc = desc
        self.is_committed = False

    async def set_config(
        self,
        *,
        author: str | None = None,
        annotations: dict[str, str] | None = None,
        labels: dict[str, str] | None = None,
        env: dict[str, str] | None = None,
    ) -> None:
        args: list[str] = []

        if author:
            args.extend(["--author", author])

        if annotations:
            for key, value in annotations.items():
                args.extend(["--annotation", f"{key}={value}"])

        if labels:
            for key, value in labels.items():
                args.extend(["--label", f"{key}={value}"])

        if env:
            for key, value in env.items():
                args.extend(["--env", f"{key.upper()}={value}"])

        if len(args) == 0:
            log.warning("set config called without arguments")
            return

        cmd = ["config"] + args
        try:
            rc, _, stderr = await _buildah_run(cmd, cid=self.cid)
        except BuildahError as e:
            msg = f"error setting config for '{self.cid}': {e}"
            log.error(msg)
            raise BuildahError(msg)

        if rc != 0:
            msg = f"error setting config for '{self.cid}': {stderr}"
            log.error(msg)
            raise BuildahError(msg)

    async def copy(self, source: Path, dest: str) -> None:
        log.debug(f"copy from '{source}' to '{dest}'")
        cmd = ["copy"]
        args = [source.resolve().as_posix(), dest]
        try:
            rc, _, stderr = await _buildah_run(
                cmd, cid=self.cid, args=args, with_args_divider=False
            )
        except (BuildahError, Exception) as e:
            msg = f"error copying '{source}' to '{dest}': {e}"
            log.error(msg)
            raise BuildahError(msg)

        if rc != 0:
            msg = f"error copying '{source}' to '{dest}': {stderr}"
            log.error(msg)
            raise BuildahError(msg)

    async def run(self, args: list[str]) -> None:
        def _out(s: str) -> None:
            log.debug(s)

        log.debug(f"run '{args}'")
        cmd = ["run", "--isolation", "chroot"]
        try:
            rc, _, stderr = await _buildah_run(
                cmd, cid=self.cid, args=args, with_args_divider=True, outcb=_out
            )
            pass
        except (BuildahError, Exception) as e:
            msg = f"error running command: {e}"
            log.error(msg)
            raise BuildahError(msg)

        if rc != 0:
            msg = f"error running command: {stderr}"
            log.error(msg)
            raise BuildahError(msg)

        pass

    async def finish(self, secrets: SecretsVaultMgr) -> None:
        # output to logger
        def _out(s: str) -> None:
            log.debug(s)

        creation_time = dt.now(tz.utc).isoformat(timespec="seconds")
        registry = self.version_desc.image.registry
        name = self.version_desc.image.name
        tag = self.version_desc.image.tag

        url = f"{registry}/{name}:{tag}"
        log.info(f"finish building container '{url}'")
        try:
            await self.set_config(
                annotations={
                    "org.opencontainers.image.created": creation_time,
                    "org.opencontainers.image.url": url,
                    "org.opencontainers.image.version": self.version_desc.version,
                }
            )
        except BuildahError as e:
            msg = (
                f"error setting final config on '{self.cid}' "
                + f"for '{self.version_desc.version}': {e}"
            )
            log.error(msg)
            raise BuildahError(msg)

        # commit container as image
        try:
            rc, _, stderr = await _buildah_run(
                ["commit", "--squash"],
                cid=self.cid,
                args=[url],
            )
        except BuildahError as e:
            msg = (
                f"error committing container '{self.cid}' for "
                + f"'{self.version_desc.version}': {e}"
            )
            log.error(msg)
            raise BuildahError(msg)

        if rc != 0:
            msg = (
                f"error committing container '{self.cid}' for "
                + f"'{self.version_desc.version}: {stderr}"
            )
            log.error(msg)
            raise BuildahError(msg)

        # obtain registry credentials
        try:
            _, username, password = secrets.harbor_creds()
            pass
        except SecretsVaultError as e:
            msg = f"error obtaining harbor credentials to push '{url}': {e}"
            log.error(msg)
            raise BuildahError(msg)

        # push to registry
        #
        log.info(f"pushing image '{url}'")

        digest_file_fd, digest_file = tempfile.mkstemp(text=True)
        try:
            rc, _, stderr = await _buildah_run(
                [
                    "push",
                    "--creds",
                    f"{username}:{password}",
                    "--digestfile",
                    digest_file,
                    url,
                ],
                outcb=_out,
            )

            with Path(digest_file).open("r") as f:
                image_digest = f.read().strip()

            log.debug(f"pushed '{url}', digest: {image_digest}")

        except BuildahError as e:
            msg = f"error pushing image '{url}': {e}"
            log.error(msg)
            raise BuildahError(msg)
        finally:
            os.close(digest_file_fd)
            os.unlink(digest_file)

        # sign image
        #

        # try:
        #     digest = await _get_digest(url)
        # except (BuildahError, Exception) as e:
        #     msg = f"error obtaining digest for '{url}': {e}"
        #     log.error(msg)
        #     raise BuildahError(msg)
        #
        img_to_sign = f"{registry}/{name}@{image_digest}"
        try:
            await async_sign(img_to_sign, secrets)
        except (SigningError, Exception) as e:
            msg = f"error signing image '{url}': {e}"
            log.error(msg)
            raise BuildahError(msg)

        if rc != 0:
            msg = f"error signing image '{url}': {stderr}"
            log.error(msg)
            raise BuildahError(msg)


async def buildah_new_container(desc: VersionDescriptor) -> BuildahContainer:
    create_args = ["from", desc.distro]
    try:
        rc, stdout, stderr = await _buildah_run(create_args)
    except BuildahError as e:
        msg = f"error creating new container: {e}"
        log.error(msg)
        raise BuildahError(msg)

    if rc != 0:
        msg = f"error creating new container ({rc}): {stderr}"
        log.error(msg)
        raise BuildahError(msg)

    cid = stdout.strip()
    ctr = BuildahContainer(cid, desc)

    author = "Clyso <support@clyso.com>"
    try:
        await ctr.set_config(
            author=author,
            annotations={
                "org.opencontainers.image.authors": author,
                "org.opencontainers.image.documentation": "https://docs.clyso.com",
                "org.opencontainers.image.revision": "",
                "org.opencontainers.image.source": "",
            },
            labels={
                "FROM_IMAGE": desc.distro,
            },
        )
    except BuildahError as e:
        msg = f"error setting config for new container '{cid}': {e}"
        log.error(msg)
        raise BuildahError(msg)

    return ctr
