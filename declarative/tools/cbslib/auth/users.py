# CBS - auth library - users
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

from typing import Annotated

import pydantic
from cbslib.auth import AuthError, AuthNoSuchUserError
from cbslib.auth import log as parent_logger
from cbslib.auth.auth import AuthTokenInfo, CBSToken, token_create
from fastapi import Depends, HTTPException, status

log = parent_logger.getChild("users")


class User(pydantic.BaseModel):
    email: str
    name: str
    token: CBSToken


class Users:
    _users_db: dict[str, User]
    _tokens_db: dict[bytes, CBSToken]

    def __init__(self) -> None:
        self._users_db = {}
        self._tokens_db = {}

    async def create(self, email: str, name: str) -> CBSToken:
        log.info(f"create user '{email}' name '{name}'")
        if email in self._users_db:
            log.debug(f"user '{email}' already exists, return")
            return await self.get_user_token(email)

        token = token_create(email)
        log.debug(f"created token for user '{email}': {token}")

        self._tokens_db[token.token] = token
        self._users_db[email] = User(email=email, name=name, token=token)
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


_auth_users: Users | None = None


async def auth_users_init() -> None:
    global _auth_users
    _auth_users = Users()


def get_auth_users() -> Users:
    if not _auth_users:
        raise AuthError("missing auth users db!")
    return _auth_users


CBSAuthUsersDB = Annotated[Users, Depends(get_auth_users)]


async def get_user(token_info: AuthTokenInfo, users: CBSAuthUsersDB) -> User:
    try:
        return await users.get_user(token_info.user)
    except AuthNoSuchUserError:
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, "Unauthorized user")


CBSAuthUser = Annotated[User, Depends(get_user)]
