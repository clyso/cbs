# crt - db - s3
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

import datetime
import http
from collections.abc import Generator
from contextlib import contextmanager
from datetime import datetime as dt

import boto3
import pydantic
from botocore.exceptions import ClientError
from ceslib.utils.secrets import SecretsVaultError, SecretsVaultMgr
from crtlib.db.errors import (
    S3DBConflictingManifestError,
    S3DBCredsError,
    S3DBError,
    S3DBExistingManifestError,
)
from crtlib.db.remote import RemoteDB
from crtlib.db.sync import sync_last_updated, sync_remote, sync_update_state
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patchset import GitHubPullRequest, PatchSet, PatchSetBase
from types_boto3_s3.client import S3Client

from . import (
    S3_DB_BUCKET,
    S3_DB_PATH,
    S3ContentType,
    get_gh_prs_loc,
    get_patchsets_loc,
    manifest_loc,
)
from . import logger as parent_logger

logger = parent_logger.getChild("s3db")


class S3Creds(pydantic.BaseModel):
    host: str
    access_key: str
    secret_key: str


class KVObjectEntry(pydantic.BaseModel):
    obj_key: str
    etag: str


def _get_s3_error(e: ClientError) -> int | None:
    if (
        "ResponseMetadata" in e.response
        and "HTTPStatusCode" in e.response["ResponseMetadata"]
    ):
        return e.response["ResponseMetadata"]["HTTPStatusCode"]
    return None


class S3DB:
    _remote_db: RemoteDB
    _s3_creds: S3Creds

    def __init__(self, remote_db: RemoteDB, secrets: SecretsVaultMgr) -> None:
        self._remote_db = remote_db

        try:
            host, access_key, secret_key = secrets.s3_creds()
        except SecretsVaultError as e:
            msg = f"unable to obtain S3 creds from vault: {e}"
            logger.error(msg)
            raise S3DBCredsError(msg=msg) from None

        self._s3_creds = S3Creds(
            host=host, access_key=access_key, secret_key=secret_key
        )

    @contextmanager
    def with_s3_client(self) -> Generator[S3Client]:
        client = boto3.client(  # pyright: ignore[reportUnknownMemberType]
            "s3",
            endpoint_url=f"https://{self._s3_creds.host}",
            aws_access_key_id=self._s3_creds.access_key,
            aws_secret_access_key=self._s3_creds.secret_key,
        )
        yield client
        client.close()

    @property
    def is_synced(self) -> bool:
        with self.with_s3_client() as s3:
            return sync_last_updated(s3) == self._remote_db.last_updated

    def sync(self) -> None:
        with self.with_s3_client() as s3:
            try:
                sync_remote(s3, self._remote_db)
            except S3DBError as e:
                logger.error(f"error synchronizing remote: {e}")
                raise e from None

    def _update_state(self, s3: S3Client) -> None:
        """Update S3 sync state."""
        sync_update_state(s3, updated_on=dt.now(datetime.UTC))

    def publish_manifest(
        self,
        manifest: ReleaseManifest,
        *,
        orig_etag: str | None,
    ) -> None:
        """Publish a manifest to S3."""
        publish_loc = manifest_loc(S3_DB_PATH, manifest.release_uuid)

        manifest_json = manifest.model_dump_json(indent=2).encode()

        with self.with_s3_client() as s3:
            try:
                # we need to do this branching because we haven't figured out how to
                # optionally passing keyword arguments to boto3 in a way that it
                # actually works. `None` is not supported (types expect a typed dict),
                # and an empty string is considered an actual parameter.
                #
                # NOTE: please try fixing this at some point, even if hope may be none.

                if orig_etag:
                    res = s3.put_object(
                        Bucket=S3_DB_BUCKET,
                        Key=publish_loc,
                        Body=manifest_json,
                        ContentType=S3ContentType.MANIFEST,
                        IfMatch=orig_etag,
                    )
                else:
                    res = s3.put_object(
                        Bucket=S3_DB_BUCKET,
                        Key=publish_loc,
                        Body=manifest_json,
                        ContentType=S3ContentType.MANIFEST,
                        IfNoneMatch="*",
                    )
            except s3.exceptions.ClientError as e:
                reason: str | None = None
                rc = _get_s3_error(e)
                exc = S3DBError
                if rc and rc == http.HTTPStatus.PRECONDITION_FAILED:
                    if orig_etag:
                        reason = "etag mismatch on existing manifest"
                        exc = S3DBConflictingManifestError
                    else:
                        reason = "unexpected existing manifest"
                        exc = S3DBExistingManifestError
                reason = reason if reason else str(e)
                msg = (
                    f"unable to put manifest uuid '{manifest.release_uuid}' "
                    + f"object to '{publish_loc}': {reason}"
                )
                logger.error(msg)
                raise exc(msg=msg) from None
            self._update_state(s3)

        logger.debug(f"published manifest result:\n{res}")

    def publish_gh_patchset(self, patchset: GitHubPullRequest) -> None:
        """Publish a GitHub pull request patch set to S3."""
        patchsets_loc = get_patchsets_loc(S3_DB_PATH)
        gh_prs_loc = get_gh_prs_loc(S3_DB_PATH)
        pr_desc = f"{patchset.org_name}/{patchset.repo_name}/{patchset.pull_request_id}"
        pr_loc = f"{gh_prs_loc}/{pr_desc}"

        patchset_uuid_str = str(patchset.patchset_uuid)
        patchset_loc = f"{patchsets_loc}/{patchset.patchset_uuid}.json"

        with self.with_s3_client() as s3:
            try:
                _ = s3.head_object(Bucket=S3_DB_BUCKET, Key=pr_loc)
            except s3.exceptions.ClientError as e:
                rc = _get_s3_error(e)
                if not rc or rc != http.HTTPStatus.NOT_FOUND:
                    msg = f"unexpected error obtaining object '{pr_loc}' head: {e}"
                    logger.error(msg)
                    raise S3DBError(msg=msg) from None
            else:
                logger.info(f"github patch set '{pr_desc}' already published")
                return

            try:
                _ = s3.head_object(Bucket=S3_DB_BUCKET, Key=patchset_loc)
            except s3.exceptions.ClientError as e:
                rc = _get_s3_error(e)
                if not rc or rc != http.HTTPStatus.NOT_FOUND:
                    msg = f"unexpected error obtaining object '{patchset_loc}': {e}"
                    logger.error(msg)
                    raise S3DBError(msg=msg) from None
            else:
                msg = (
                    f"unexpected uuid '{patchset.patchset_uuid}' at '{patchset_loc}' "
                    + f"found (potentially for gh pr '{pr_desc}')"
                )
                logger.error(msg)
                raise S3DBError(msg=msg)

            patchset_json = PatchSet(info=patchset).model_dump_json(indent=2).encode()

            try:
                _ = s3.put_object(
                    Bucket=S3_DB_BUCKET,
                    Key=pr_loc,
                    Body=patchset_uuid_str,
                    ContentType=S3ContentType.UUID,
                    IfNoneMatch="*",
                )
            except s3.exceptions.ClientError as e:
                reason = str(e)
                rc = _get_s3_error(e)
                exc = S3DBError
                if rc and rc == http.HTTPStatus.PRECONDITION_FAILED:
                    reason = f"unexpected existing github pr '{pr_desc}'"
                msg = f"unable to put github pr '{pr_desc}': {reason}"
                logger.error(msg)
                raise exc(msg=msg) from None

            try:
                _ = s3.put_object(
                    Bucket=S3_DB_BUCKET,
                    Key=patchset_loc,
                    Body=patchset_json,
                    ContentType=S3ContentType.PATCHSET,
                    IfNoneMatch="*",
                )
            except s3.exceptions.ClientError as e:
                reason = str(e)
                rc = _get_s3_error(e)
                exc = S3DBError
                if rc and rc == http.HTTPStatus.PRECONDITION_FAILED:
                    reason = (
                        f"unexpected existing patch set uuid '{patchset.patchset_uuid}'"
                    )
                msg = f"unable to put patch set '{patchset.patchset_uuid}': {reason}"
                logger.error(msg)
                raise exc(msg=msg) from None

            self._update_state(s3)

        logger.info(
            f"published patch set uuid '{patchset.patchset_uuid}' gh pr '{pr_desc}'"
        )

    def publish_vanilla_patchset(self, patchset: PatchSetBase) -> None:
        logger.warning(
            "publishing vanilla patch sets is not implemented! "
            + f"(uuid '{patchset.patchset_uuid}')"
        )
        raise NotImplementedError()
