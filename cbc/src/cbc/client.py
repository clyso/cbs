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
from typing import Any, override

import httpx
import pydantic
from httpx import _types as httpx_types  # pyright: ignore[reportPrivateUsage]

from cbc import CBCError
from cbsdcore.api.responses import BaseErrorModel


class CBCConnectionError(CBCError):
    @override
    def __str__(self) -> str:
        return "Connection Error" + (f": {self.msg}" if self.msg else "")


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

    def _maybe_handle_error(self, res: httpx.Response) -> None:
        if res.is_error:
            try:
                err = BaseErrorModel.model_validate(res.json())
                msg = err.detail
            except pydantic.ValidationError:
                msg = res.read().decode("utf-8")

            raise CBCError(msg)

    def get(
        self, ep: str, *, params: httpx_types.QueryParamTypes | None = None
    ) -> httpx.Response:
        try:
            res = self._client.get(ep, params=params)
            self._maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.error(msg)
            raise CBCConnectionError(msg) from e
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
        try:
            res = self._client.post(ep, json=data)  # pyright: ignore[reportAny]
            self._maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.error(msg)
            raise CBCConnectionError(msg) from e
        except Exception as e:
            msg = f"error posting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        return res

    def delete(
        self, ep: str, params: httpx_types.QueryParamTypes | None = None
    ) -> httpx.Response:
        try:
            res = self._client.delete(ep, params=params)
            self._maybe_handle_error(res)
        except httpx.ConnectError as e:
            msg = f"error connecting to '{self._client.base_url}': {e}"
            self._logger.error(msg)
            raise CBCConnectionError(msg) from e
        except Exception as e:
            msg = f"error deleting '{ep}': {e}"
            self._logger.error(msg)
            raise CBCError(msg) from e
        return res
