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

import dbm
from pathlib import Path
from typing import Annotated, override

import pydantic
from cbscore.errors import CESError
from fastapi import Depends, HTTPException, status

from cbslib.auth import AuthError, AuthNoSuchUserError
from cbslib.auth import logger as parent_logger
from cbslib.auth.auth import AuthTokenInfo, CBSToken, token_create
from cbslib.config import get_config

logger = parent_logger.getChild("users")


class AuthUsersDBMissingError(AuthError):
    """Auth Users DB is missing."""

    def __init__(self) -> None:
        super().__init__("missing auth users db!")


class UsersDBError(CESError):
    @override
    def __str__(self) -> str:
        return "Users DB Error" + (f": {self.msg}" if self.msg else "")


class User(pydantic.BaseModel):
    email: str
    name: str
    token: CBSToken


class Users:
    _db_path: Path
    _users_db: dict[str, User]
    _tokens_db: dict[bytes, CBSToken]

    def __init__(self, db_path: Path) -> None:
        self._db_path = db_path
        self._users_db = {}
        self._tokens_db = {}

    async def create(self, email: str, name: str) -> CBSToken:
        logger.info(f"create user '{email}' name '{name}'")
        if email in self._users_db:
            logger.debug(f"user '{email}' already exists, return")
            return await self.get_user_token(email)

        token = token_create(email)
        logger.debug(f"created token for user '{email}': {token}")

        self._tokens_db[token.token] = token
        self._users_db[email] = User(email=email, name=name, token=token)
        await self.save()
        return token

    async def get_user_token(self, email: str) -> CBSToken:
        if email not in self._users_db:
            raise AuthNoSuchUserError(email)
        user = self._users_db[email]
        return user.token

    async def get_user(self, email: str) -> User:
        if email not in self._users_db:
            raise AuthNoSuchUserError(email)
        return self._users_db[email]

    async def load(self) -> None:
        try:
            with dbm.open(self._db_path, "c") as db:
                if "users" in db:
                    users_adapter = pydantic.TypeAdapter(dict[str, User])
                    self._users_db = users_adapter.validate_json(db["users"])

                for user in self._users_db.values():
                    self._tokens_db[user.token.token] = user.token
        except Exception as e:
            msg = f"error loading users from db '{self._db_path}': {e}"
            logger.exception(msg)
            raise UsersDBError(msg) from e

        logger.info(f"loaded {len(self._users_db)} users from database")

    async def save(self) -> None:
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
    _auth_users = Users(config.server.db_path)
    await _auth_users.load()


def get_auth_users() -> Users:
    if not _auth_users:
        raise AuthUsersDBMissingError()
    return _auth_users


CBSAuthUsersDB = Annotated[Users, Depends(get_auth_users)]


async def get_user(token_info: AuthTokenInfo, users: CBSAuthUsersDB) -> User:
    try:
        return await users.get_user(token_info.user)
    except AuthNoSuchUserError:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, "Unauthorized user") from None


CBSAuthUser = Annotated[User, Depends(get_user)]
