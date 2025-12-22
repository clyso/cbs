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

import errno
import sys
import threading
from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any

import uvicorn
from cbscore.errors import CESError
from cbslib.auth.oauth import oauth_init
from cbslib.auth.users import auth_users_init
from cbslib.config.config import config_init
from cbslib.core.mgr import MgrError, mgr_init
from cbslib.core.monitor import monitor
from cbslib.logger import logger as parent_logger
from cbslib.logger import setup_logging, uvicorn_logging_config
from cbslib.routes import auth, builds, components
from fastapi import FastAPI
from starlette.middleware.sessions import SessionMiddleware

logger = parent_logger.getChild("server")


# fastapi application
#
@asynccontextmanager
async def lifespan(_: FastAPI) -> AsyncGenerator[None, Any]:
    logger.info("Preparing cbs service server...")

    try:
        await auth_users_init()
    except (CESError, Exception):
        logger.error("error initializing users db")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        oauth_init()
    except (CESError, Exception):
        logger.error("error initiating server")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        mgr = mgr_init()
    except (MgrError, Exception) as e:
        logger.error(f"error initializing manager: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    thread = threading.Thread(target=monitor, args=(mgr.tracker,))
    thread.start()

    logger.info("Starting cbs service server...")
    yield
    logger.info("Shutting down cbs service server...")
    thread.join()


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
        logger.debug("init config")
        config = config_init()
    except Exception:
        logger.exception("error setting up config state")
        sys.exit(1)

    if not config.server:
        logger.error("missing server config")
        sys.exit(errno.EINVAL)

    api.add_middleware(
        SessionMiddleware, secret_key=config.server.secrets.session_secret_key
    )

    api.include_router(auth.router)
    api.include_router(auth.permissions_router)
    api.include_router(builds.router)
    api.include_router(components.router)
    app.mount("/api", api)

    return app


# main
#
def main() -> None:
    ourname = Path(__file__).stem
    config = config_init()
    if not config.server:
        print("missing server config")
        sys.exit(errno.EINVAL)

    uvicorn.run(
        app=f"{ourname}:factory",
        host="0.0.0.0",  # noqa: S104
        port=8080,
        factory=True,
        log_config=uvicorn_logging_config(),
        ssl_certfile=config.server.cert,
        ssl_keyfile=config.server.key,
    )


if __name__ == "__main__":
    main()
