# cbc - client
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

import logging
import re
from collections.abc import Generator
from contextlib import contextmanager
from typing import Any, override
from urllib.parse import unquote

import httpx
import pydantic
from cbsdcore.api.responses import BaseErrorModel
from httpx import _types as httpx_types  # pyright: ignore[reportPrivateUsage]

from cbc import CBCError


class CBCConnectionError(CBCError):
    """Connection error to the CBS server."""

    @override
    def __str__(self) -> str:
        return "Connection error" + (f": {self.msg}" if self.msg else "")


class CBCPermissionDeniedError(CBCError):
    """Permission denied from the CBS server, most likely due to invalid token."""

    @override
    def __str__(self) -> str:
        return "Permission denied" + (f": {self.msg}" if self.msg else "")


QueryParams = httpx_types.QueryParamTypes


# Generated using GitHub Copilot, Claude Code Sonnet 4.5
#   on Jan 17 2026, by Joao Eduardo Luis <joao@clyso.com>
#
# Edited to make ajust to our needs.
#
def _get_download_filename(content_disposition: str) -> str | None:
    """Extract filename from Content-Disposition header."""
    if not content_disposition:
        return None

    # Try RFC 5987 filename* first (e.g., filename*=UTF-8''file%20name.pdf)
    match = re.search(
        r"filename\*=([^']+)''(.+?)(?:;|$)", content_disposition, re.IGNORECASE
    )
    if match:
        encoding = match.group(1) or "utf-8"
        return unquote(match.group(2), encoding=encoding)

    # Fall back to regular filename (e.g., filename="file.pdf" or filename=file.pdf)
    match = re.search(
        r'filename=(["\']?)(.+?)\1(?:;|$)', content_disposition, re.IGNORECASE
    )
    if match:
        return match.group(2).strip("'\"")

    return None


class CBCClient:
    _client: httpx.Client
    _logger: logging.Logger

    def __init__(
        self,
        logger: logging.Logger,
        base_url: str,
        *,
        token: str | None = None,
        verify: bool = False,
    ) -> None:
        self._logger = logger

        headers = None if not token else {"Authorization": f"Bearer {token}"}

        self._client = httpx.Client(
            base_url=f"{base_url}/api",
            headers=headers,
            verify=verify,
        )

    def maybe_handle_error(self, res: httpx.Response) -> None:
        if not res.is_error:
            return

        res_value = res.read()

        try:
            err = BaseErrorModel.model_validate_json(res_value)
            msg = err.detail
        except pydantic.ValidationError:
            msg = res.read().decode("utf-8")

        raise CBCError(msg)

    @contextmanager
    def download(self, ep: str) -> Generator[tuple[str | None, httpx.Response]]:
        try:
            with self._client.stream("GET", ep) as response:
                filename: str | None = None
                if content_disposition := response.headers.get("content-disposition"):
                    filename = _get_download_filename(content_disposition)

                yield (filename, response)

        except Exception as e:
            raise CBCError(f"error downloading file: {e}") from e

    def get(self, ep: str, *, params: QueryParams | None = None) -> httpx.Response:
        """Send a GET request to the given CBS endpoint."""
        try:
            res = self._client.get(ep, params=params)
            self.maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.error(msg)
            raise CBCConnectionError(msg) from e
        except httpx.HTTPStatusError as e:
            if (
                e.response.status_code == httpx.codes.UNAUTHORIZED.value
                or e.response.status_code == httpx.codes.FORBIDDEN.value
            ):
                msg = f"authentication error accessing '{ep}': {e}"
                self._logger.error(msg)
                raise CBCPermissionDeniedError(msg) from e
            msg = f"error getting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        except Exception as e:
            msg = f"error getting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        return res

    def post(
        self,
        ep: str,
        data: Any,  # pyright: ignore[reportExplicitAny, reportAny]
    ) -> httpx.Response:
        """Send a POST request to the given CBS endpoint."""
        try:
            res = self._client.post(ep, json=data)  # pyright: ignore[reportAny]
            self.maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.error(msg)
            raise CBCConnectionError(msg) from e
        except httpx.HTTPStatusError as e:
            if (
                e.response.status_code == httpx.codes.UNAUTHORIZED.value
                or e.response.status_code == httpx.codes.FORBIDDEN.value
            ):
                msg = f"authentication error accessing '{ep}': {e}"
                self._logger.error(msg)
                raise CBCPermissionDeniedError(msg) from e
            msg = f"error getting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        except Exception as e:
            msg = f"error posting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        return res

    def put(
        self,
        ep: str,
        *,
        data: Any | None = None,  # pyright: ignore[reportExplicitAny]
    ) -> httpx.Response:
        """Send a PUT request to the given CBS endpoint."""
        try:
            res = self._client.put(ep, json=data)
            self.maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.error(msg)
            raise CBCConnectionError(msg) from e
        except httpx.HTTPStatusError as e:
            if (
                e.response.status_code == httpx.codes.UNAUTHORIZED.value
                or e.response.status_code == httpx.codes.FORBIDDEN.value
            ):
                msg = f"authentication error accessing '{ep}': {e}"
                self._logger.error(msg)
                raise CBCPermissionDeniedError(msg) from e
            msg = f"error getting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        except Exception as e:
            msg = f"error putting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        return res

    def delete(self, ep: str, params: QueryParams | None = None) -> httpx.Response:
        """Send a DELETE request to the given CBS endpoint."""
        try:
            res = self._client.delete(ep, params=params)
            self.maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.error(msg)
            raise CBCConnectionError(msg) from e
        except httpx.HTTPStatusError as e:
            if (
                e.response.status_code == httpx.codes.UNAUTHORIZED.value
                or e.response.status_code == httpx.codes.FORBIDDEN.value
            ):
                msg = f"authentication error accessing '{ep}': {e}"
                self._logger.error(msg)
                raise CBCPermissionDeniedError(msg) from e
            msg = f"error getting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        except Exception as e:
            msg = f"error deleting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        return res
