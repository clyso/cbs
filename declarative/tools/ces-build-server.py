#!/usr/bin/env python3

# Serves build capabilities over a REST API
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

# pyright: reportExplicitAny=false

from __future__ import annotations

import sys
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from typing import Any

import uvicorn
import uvicorn.config
from cbslib.auth.oauth import oauth_init
from cbslib.auth.users import auth_users_init

from cbslib.config.server import config_init
from cbslib.logger import log as parent_logger
from cbslib.logger import setup_logging, uvicorn_logging_config
from cbslib.routes import auth, builds
from ceslib.errors import CESError
from fastapi import FastAPI
from starlette.middleware.sessions import SessionMiddleware

log = parent_logger.getChild("server")


# fastapi application
#
@asynccontextmanager
async def lifespan(_: FastAPI) -> AsyncGenerator[None, Any]:
    log.info("Preparing server init")

    try:
        await auth_users_init()
    except (CESError, Exception) as e:
        log.error(f"error initializing users db: {e}")
        sys.exit(1)

    try:
        oauth_init()
    except (CESError, Exception) as e:
        log.error(f"error initiating server: {e}")
        sys.exit(1)

    log.info("Starting ces build server")
    yield
    log.info("Shutting down ces build server")


def factory() -> FastAPI:
    api_tags_meta = [
        {
            "name": "versions",
            "description": "Versions related operations",
        }
    ]

    app = FastAPI(docs_url=None, lifespan=lifespan)
    api = FastAPI(
        title="CES builder API",
        description="CES release builder",
        version="1.0.0",
        openapi_tags=api_tags_meta,
    )

    setup_logging()

    try:
        log.debug("init config")
        config = config_init()
    except Exception as e:
        log.error(f"error setting up config state: {e}")
        sys.exit(1)

    api.add_middleware(
        SessionMiddleware, secret_key=config.secrets.server.session_secret_key
    )

    api.include_router(auth.router)
    api.include_router(builds.router)
    app.mount("/api", api)

    return app


# uvicorn logging setup
#


# main
#
def main() -> None:
    config = config_init()

    uvicorn.run(
        app="ces-build-server:factory",
        host="0.0.0.0",
        port=8080,
        factory=True,
        log_config=uvicorn_logging_config(),
        ssl_certfile=config.server.cert_path,
        ssl_keyfile=config.server.key_path,
    )


if __name__ == "__main__":
    main()
