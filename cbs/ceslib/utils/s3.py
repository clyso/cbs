# CES library - S3 utils
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
from pathlib import Path
from typing import override

import aioboto3
import pydantic
from ceslib.errors import CESError
from ceslib.utils import log as parent_logger
from ceslib.utils.secrets import SecretsVaultError, SecretsVaultMgr
from types_aiobotocore_s3.service_resource import S3ServiceResource


class S3Error(CESError):
    @override
    def __str__(self) -> str:
        return "S3 Error" + ("" if not self.msg else f": {self.msg}")


class S3FileLocator:
    src: Path
    dst: str
    name: str

    def __init__(self, src: Path, dst: str, name: str) -> None:
        self.src = src
        self.dst = dst
        self.name = name


class S3ObjectEntry(pydantic.BaseModel):
    """Represent an object in S3."""

    key: str
    size: int
    last_modified: dt

    @property
    def name(self) -> str:
        idx = self.key.rfind("/")
        if idx < 0:
            return self.key
        return self.key[idx + 1 :]


class S3ListResult(pydantic.BaseModel):
    """Result from listing objects in an S3 bucket."""

    objects: list[S3ObjectEntry]
    common_prefixes: list[str]


_UPLOAD_BUCKET = "ces-packages"


log = parent_logger.getChild("s3")


async def s3_upload_str_obj(
    secrets: SecretsVaultMgr,
    location: str,
    contents: str,
    content_type: str = "application/json",
) -> None:
    """
    Upload a string object to S3.

    If not specified, presumes the object's content is a JSON string.
    """
    try:
        hostname, access_id, secret_id = secrets.s3_creds()
    except SecretsVaultError as e:
        msg = f"error obtaining S3 credentials: {e}"
        log.exception(msg)
        raise S3Error(msg) from e

    log.debug(f"S3: hostname = {hostname}, access_id = {access_id}")

    s3_session = aioboto3.Session(
        aws_access_key_id=access_id,
        aws_secret_access_key=secret_id,
    )

    if not hostname.startswith("http"):
        hostname = f"https://{hostname}"

    async with s3_session.resource("s3", None, None, True, True, hostname) as s3:
        bucket = await s3.Bucket(_UPLOAD_BUCKET)
        try:
            _ = await bucket.put_object(
                Key=location,
                Body=contents,
                ContentType=content_type,
            )
        except Exception as e:
            msg = f"error uploading object to '{location}': {e}"
            log.exception(msg)
            raise S3Error(msg) from e


async def s3_download_str_obj(
    secrets: SecretsVaultMgr,
    location: str,
    content_type: str | None = None,
) -> str | None:
    """
    Download a string object from S3.

    If not specified, presumes the object's content is JSON.
    """
    try:
        hostname, access_id, secret_id = secrets.s3_creds()
    except SecretsVaultError as e:
        msg = f"error obtaining S3 credentials: {e}"
        log.exception(msg)
        raise S3Error(msg) from e

    log.debug(f"S3: hostname = {hostname}, access_id = {access_id}")

    s3_session = aioboto3.Session(
        aws_access_key_id=access_id,
        aws_secret_access_key=secret_id,
    )

    if not hostname.startswith("http"):
        hostname = f"https://{hostname}"

    async with s3_session.resource("s3", None, None, True, True, hostname) as s3:
        bucket = await s3.Bucket(_UPLOAD_BUCKET)
        try:
            obj = await bucket.Object(location)
        except s3.meta.client.exceptions.NoSuchKey:
            log.debug(f"object '{location}' not found")
            return None
        except Exception as e:
            msg = f"error downloading string object from '{location}': {e}"
            log.exception(msg)
            raise S3Error(msg) from e

        obj_content_type = await obj.content_type
        if content_type and obj_content_type != content_type:
            msg = f"unexpected content type '{obj_content_type}' for string object"
            log.error(msg)
            raise S3Error(msg)

        contents = await obj.get()
        body = contents["Body"]

        try:
            data = await body.read()
        except Exception as e:
            msg = f"error reading object string from '{location}': {e}"
            log.exception(msg)
            raise S3Error(msg) from e

        return data.decode("utf-8")


async def s3_upload_json(
    secrets: SecretsVaultMgr, location: str, contents: str
) -> None:
    """Upload a JSON object."""
    return await s3_upload_str_obj(
        secrets, location, contents, content_type="application/json"
    )


async def s3_download_json(secrets: SecretsVaultMgr, location: str) -> str | None:
    """Download a JSON object."""
    return await s3_download_str_obj(secrets, location, content_type="application/json")


async def _upload_file(
    s3: S3ServiceResource,
    file_loc: S3FileLocator,
    public: bool = False,
) -> None:
    """Upload a file from the local filesystem to S3."""
    bucket = await s3.Bucket(_UPLOAD_BUCKET)

    extra_args = None if not public else {"ACL": "public-read"}

    log.debug(f"uploading file '{file_loc.name}' to '{file_loc.dst}'")
    try:
        await bucket.upload_file(
            file_loc.src.as_posix(),
            Key=file_loc.dst,
            ExtraArgs=extra_args,
        )
    except Exception as e:
        msg = (
            f"error uploading file '{file_loc.name}' from '{file_loc.src}' "
            + f"to '{file_loc.dst}': {e}"
        )
        log.exception(msg)
        raise S3Error(msg) from e


async def s3_upload_files(
    secrets: SecretsVaultMgr, file_locs: list[S3FileLocator]
) -> None:
    """Upload a list of files to S3."""
    try:
        hostname, access_id, secret_id = secrets.s3_creds()
    except SecretsVaultError as e:
        msg = f"error obtaining S3 credentials: {e}"
        log.exception(msg)
        raise S3Error(msg) from e

    log.debug(f"S3: hostname = {hostname}, access_id = {access_id}")

    s3_session = aioboto3.Session(
        aws_access_key_id=access_id,
        aws_secret_access_key=secret_id,
    )

    if not hostname.startswith("http"):
        hostname = f"https://{hostname}"

    async with s3_session.resource("s3", None, None, True, True, hostname) as s3:
        for loc in file_locs:
            try:
                await _upload_file(s3, loc)
            except S3Error as e:
                msg = f"error uploading file: {e}"
                log.exception(msg)
                raise S3Error(msg) from e
            except Exception as e:
                msg = f"unknown error uploading file: {e}"
                log.exception(msg)
                raise S3Error(msg) from e


async def s3_list(
    secrets: SecretsVaultMgr,
    *,
    prefix: str | None = None,
    prefix_as_directory: bool = False,
) -> S3ListResult:
    """
    List objects in S3.

    If `prefix` is provided, list only objects with said prefix.
    If `prefix_as_directory` is provided, ensure that the "/" delimiter is used to
    differentiate between objects under `prefix` and those under a (logical)
    sub-directory.

    Returns `S3ListResult`, containing the list of objects and `common_prefixes`
    representing those other (logical) sub-directories present in `prefix`.
    """
    try:
        hostname, access_id, secret_id = secrets.s3_creds()
    except SecretsVaultError as e:
        msg = f"error obtaining S3 credentials: {e}"
        log.exception(msg)
        raise S3Error(msg) from e

    s3_session = aioboto3.Session(
        aws_access_key_id=access_id,
        aws_secret_access_key=secret_id,
    )
    if not hostname.startswith("http"):
        hostname = f"https://{hostname}"

    obj_lst: list[S3ObjectEntry] = []
    common_prefixes: set[str] = set()

    if prefix_as_directory and not prefix:
        prefix_as_directory = False

    delimiter = "" if not prefix_as_directory else "/"

    async with (
        s3_session.client("s3", endpoint_url=hostname) as s3_client,
    ):
        log.debug(f"listing objects for bucket '{_UPLOAD_BUCKET}")

        continuation_token = ""
        while True:
            log.debug(f"listing objects, continuation_token: '{continuation_token}'")
            res = await s3_client.list_objects_v2(
                Bucket=_UPLOAD_BUCKET,
                Prefix=prefix if prefix else "",
                Delimiter=delimiter,
                ContinuationToken=continuation_token,
            )

            common_prefixes_dict = res.get("CommonPrefixes")
            if common_prefixes_dict:
                for entry in common_prefixes_dict:
                    p = entry.get("Prefix")
                    if p:
                        common_prefixes.add(p)

            log.debug(f"found common_prefixes: {common_prefixes}")

            objs = res.get("Contents")
            if not objs:
                break

            log.debug(f"found objects: {len(objs)}")

            for obj_entry in objs:
                key = obj_entry.get("Key")
                size = obj_entry.get("Size")
                last_modified = obj_entry.get("LastModified")
                assert key is not None
                assert size is not None
                assert last_modified is not None

                obj_lst.append(
                    S3ObjectEntry(key=key, size=size, last_modified=last_modified)
                )

            if res["IsTruncated"]:
                continuation_token = res["NextContinuationToken"]
            else:
                break

    return S3ListResult(objects=obj_lst, common_prefixes=list(common_prefixes))
