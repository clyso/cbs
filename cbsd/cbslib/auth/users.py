# CBS server library - auth library - users
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

import datetime
import dbm
from datetime import datetime as dt
from pathlib import Path
from typing import Annotated, override

import aiorwlock
import pydantic
from cbscore.errors import CESError
from cbsdcore.auth.token import Token
from cbsdcore.auth.user import User
from fastapi import Depends

from cbslib.auth import AuthError, AuthNoSuchUserError
from cbslib.auth import logger as parent_logger
from cbslib.auth.auth import token_create
from cbslib.config.config import get_config

logger = parent_logger.getChild("users")

_USERS_DB_FILE = "users.db"


class AuthUsersDBMissingError(AuthError):
    """Auth Users DB is missing."""

    def __init__(self) -> None:
        super().__init__("missing auth users db!")


class ExpiredUserTokenError(AuthError):
    """Auth user token has expired."""

    def __init__(self, user: str) -> None:
        super().__init__(f"token for user '{user}' has expired")


class UsersDBError(CESError):
    @override
    def __str__(self) -> str:
        return "Users DB Error" + (f": {self.msg}" if self.msg else "")


class Users:
    _db_path: Path
    _users_db: dict[str, User]
    _tokens_db: dict[bytes, Token]
    _rwlock: aiorwlock.RWLock

    def __init__(self, db_path: Path) -> None:
        if not db_path.exists():
            db_path.mkdir(parents=True)

        if not db_path.is_dir():
            msg = "database path is not a directory"
            logger.error(msg)
            raise UsersDBError(msg)

        self._db_path = db_path / _USERS_DB_FILE
        self._users_db = {}
        self._tokens_db = {}
        self._rwlock = aiorwlock.RWLock()

    async def create(self, email: str, name: str) -> Token:
        """Create a new user with given email and name, returning its token."""
        logger.info(f"create user '{email}' name '{name}'")

        async with self._rwlock.writer_lock:
            user = self._users_db.get(email)
            if user and (
                not user.token.info.expires
                or user.token.info.expires > dt.now(datetime.UTC)
            ):
                logger.debug(f"user '{email}' already exists, return")
                return user.token

            token = token_create(email)
            logger.debug(f"created token for user '{email}': {token}")

            self._tokens_db[token.token.get_secret_value()] = token
            self._users_db[email] = User(email=email, name=name, token=token)
            await self.save()
            return token

    async def get_user_token(self, email: str) -> Token:
        """Obtain the token for the user with given email."""
        async with self._rwlock.reader_lock:
            if email not in self._users_db:
                raise AuthNoSuchUserError(email)
            user = self._users_db[email]
            expires = user.token.info.expires
            if expires and expires < dt.now(datetime.UTC):
                raise ExpiredUserTokenError(email)
            return user.token

    async def get_user(self, email: str) -> User:
        """Obtain the user with given email."""
        async with self._rwlock.reader_lock:
            if email not in self._users_db:
                raise AuthNoSuchUserError(email)
            return self._users_db[email]

    async def load(self) -> None:
        """Load users from the database file."""
        async with self._rwlock.writer_lock:
            try:
                with dbm.open(self._db_path, "c") as db:
                    if "users" in db:
                        users_adapter = pydantic.TypeAdapter(dict[str, User])
                        self._users_db = users_adapter.validate_json(db["users"])

                    for user in self._users_db.values():
                        self._tokens_db[user.token.token.get_secret_value()] = (
                            user.token
                        )
            except Exception as e:
                msg = f"error loading users from db '{self._db_path}': {e}"
                logger.exception(msg)
                raise UsersDBError(msg) from e

            logger.info(f"loaded {len(self._users_db)} users from database")

    async def save(self) -> None:
        """Store users to the database file."""
        async with self._rwlock.writer_lock:
            try:
                with dbm.open(self._db_path, "w") as db:
                    users_adapter = pydantic.TypeAdapter(dict[str, User])
                    users_json = users_adapter.dump_json(self._users_db)
                    db["users"] = users_json
            except Exception as e:
                msg = f"error saving users to db '{self._db_path}': {e}"
                logger.exception(msg)
                raise UsersDBError(msg) from e


_auth_users: Users | None = None


async def auth_users_init() -> None:
    global _auth_users
    config = get_config()
    assert config.server, "unexpected missing server config"
    _auth_users = Users(config.server.db)
    await _auth_users.load()


def get_auth_users() -> Users:
    if not _auth_users:
        raise AuthUsersDBMissingError()
    return _auth_users


CBSAuthUsersDB = Annotated[Users, Depends(get_auth_users)]
