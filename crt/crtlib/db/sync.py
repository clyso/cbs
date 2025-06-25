# crt - releases db = sync
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

import re
from datetime import datetime as dt
from typing import cast

import pydantic
from crtlib.db import S3_DB_BUCKET, S3_DB_PATH
from crtlib.db.errors import S3DBError
from crtlib.db.remote import RemoteDB
from types_boto3_s3.client import S3Client

from . import CRT_CONTENT_TYPE_PREFIX, S3_STATE_PATH, S3ContentType
from . import logger as parent_logger

logger = parent_logger.getChild("sync")


class _SyncState(pydantic.BaseModel):
    last_updated: dt


def sync_last_updated(s3: S3Client) -> dt | None:
    """Obtain when the S3 db was last updated."""
    try:
        obj = s3.get_object(Bucket=S3_DB_BUCKET, Key=S3_STATE_PATH)
    except s3.exceptions.ClientError as e:
        if (
            "ResponseMetadata" not in e.response
            or "HTTPStatusCode" not in e.response["ResponseMetadata"]
            or e.response["ResponseMetadata"]["HTTPStatusCode"] != 404
        ):
            msg = f"unable to obtain state obj from S3: {e}"
            logger.warning(msg)
            logger.warning(e.response)
        return None

    try:
        obj_body = obj["Body"].read()
    except Exception as e:
        msg = f"unable to read state obj body: {e}"
        logger.warning(msg)
        return None

    try:
        state = _SyncState.model_validate_json(obj_body)
    except pydantic.ValidationError:
        msg = "malformed state obj"
        logger.error(msg)
        raise S3DBError(msg) from None

    return state.last_updated


def sync_update_state(s3: S3Client, *, updated_on: dt) -> None:
    """Update sync state in S3."""
    state = _SyncState(last_updated=updated_on)
    state_json = state.model_dump_json(indent=2).encode()
    try:
        _ = s3.put_object(
            Bucket=S3_DB_BUCKET,
            Key=S3_STATE_PATH,
            Body=state_json,
            ContentType=S3ContentType.STATE,
        )
    except s3.exceptions.ClientError as e:
        msg = f"unable to put sync state to '{S3_STATE_PATH}': {e}"
        logger.error(msg)
        raise S3DBError(msg=msg) from None


def _is_acceptable_content_type(s3: S3Client, obj_key: str) -> bool:
    """Check if the given object `obj_key` has an appropriate content type set."""
    try:
        res = s3.head_object(Bucket=S3_DB_BUCKET, Key=f"{S3_DB_PATH}/{obj_key}")
    except s3.exceptions.ClientError as e:
        msg = f"unable to check head for object '{obj_key}': {e}"
        logger.error(msg)
        raise S3DBError(msg=msg) from None

    return res["ContentType"].startswith(CRT_CONTENT_TYPE_PREFIX)


def sync_remote(
    s3: S3Client,
    db: RemoteDB,
) -> None:
    """
    Synchronize the remote S3 db with its local on-disk representation.

    At the moment, adds entries to the local db but will not remove any.
    """
    last_update = sync_last_updated(s3)
    if last_update is not None and last_update == db.last_updated:
        logger.info("remote db is up to date with s3 db")
        return

    try:
        objs_lst = s3.list_objects_v2(Bucket=S3_DB_BUCKET, Prefix=S3_DB_PATH)
    except s3.exceptions.ClientError as e:
        msg = f"unable to list objects in bucket '{S3_DB_BUCKET}': {e}"
        logger.error(msg)
        raise S3DBError(msg=msg) from None

    if "Contents" not in objs_lst:
        logger.debug("nothing to sync")
        return

    obj_key_re = re.compile(rf"^{S3_DB_PATH}/(.+)$")
    for entry in objs_lst["Contents"]:
        if "Key" not in entry:
            logger.warning(f"missing 'Key' for obj: {entry}")
            continue
        elif "ETag" not in entry:
            logger.warning(f"missing 'ETag' for obj: {entry}")
            continue

        m = re.match(obj_key_re, entry["Key"])
        if not m:
            logger.warning(f"object '{entry['Key']}' does not match")
            continue

        obj_key = cast(str, m.group(1))
        logger.debug(f"add obj '{obj_key}' to remote db")

        if not obj_key.endswith(".json") and not _is_acceptable_content_type(
            s3, obj_key
        ):
            logger.warning(f"object key is not an acceptable file type: {obj_key}")
            continue

        if db.exists(obj_key, etag=entry["ETag"]):
            logger.info(f"obj '{obj_key}' at latest version")
            continue

        try:
            obj = s3.get_object(Bucket=S3_DB_BUCKET, Key=entry["Key"])
        except s3.exceptions.ClientError as e:
            msg = f"unable to obtain object '{entry['Key']}': {e}"
            logger.error(msg)
            raise S3DBError(msg=msg) from None

        try:
            obj_body = obj["Body"].read()
        except Exception as e:
            msg = f"unable to read body for object '{obj_key}': {e}"
            logger.error(msg)
            raise S3DBError(msg=msg) from None

        try:
            db.update(obj_key, etag, obj_body, updated_on=obj["LastModified"])
        except Exception as e:
            msg = f"unable to write object '{obj_key}' to db: {e}"
            logger.error(msg)
            raise S3DBError(msg=msg) from None

    db.sync_to_disk(updated_on=last_update)
