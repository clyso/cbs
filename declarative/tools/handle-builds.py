#!/usr/bin/env python3

import errno
import logging
import sys
from pathlib import Path

import click

from ceslib.logging import log as root_logger
from ceslib.builds.desc import BuildDescriptor
from ceslib.utils.git import GitError, get_git_modified_paths


log = root_logger.getChild("handle-builds")


class DescriptorEntry:
    build_name: str
    build_type: str
    descriptor: BuildDescriptor

    def __init__(self, build_name: str, build_type: str, path: Path):
        self.build_name = build_name
        self.build_type = build_type
        self.descriptor = BuildDescriptor.read(path)


@click.command()
@click.option("-d", "--debug", is_flag=True)
@click.option("--base-path", envvar="BUILDS_BASE_PATH", type=str, required=True)
@click.option("--base-sha", envvar="BUILDS_BASE_SHA", type=str, required=True)
def main(
    debug: bool,
    base_path: str,
    base_sha: str,
):
    if debug:
        root_logger.setLevel(logging.DEBUG)

    log.debug(f"base_path: {base_path}, base_sha: {base_sha}")

    try:
        descs_modified, descs_deleted = get_git_modified_paths(
            base_sha, "HEAD", base_path
        )
    except GitError as e:
        log.error(f"unable to obtain modified paths: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    for desc_file in descs_deleted:
        if desc_file.suffix != ".json":
            continue
        log.info(f"descriptor for '{desc_file.name}' deleted")

    for desc_file in descs_modified:
        if desc_file.suffix != ".json":
            continue

        log.info(f"info: process descriptor for '{desc_file.name}'")
        build_name = desc_file.stem
        build_type = desc_file.parent.name
        desc = DescriptorEntry(build_name, build_type, desc_file)

        print("=> handle build descriptor:")
        print(f"-       name: {desc.build_name}")
        print(f"-       type: {desc.build_type}")
        print(f"-    version: {desc.descriptor.version}")
        print("- components:")
        for c in desc.descriptor.components:
            print(f"-- {c.name}: {c.version}")

        # TODO: call 'ces-build.sh' with info for this build


if __name__ == "__main__":
    main()
