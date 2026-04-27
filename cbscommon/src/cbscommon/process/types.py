# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Clyso GmbH

import abc
from collections.abc import Callable, Coroutine
from typing import Any

AsyncRunCmdOutCallback = Callable[[str], Coroutine[Any, Any, None]]  # pyright: ignore[reportExplicitAny]


class SecureArg(abc.ABC):
    @property
    @abc.abstractmethod
    def value(self) -> str:
        pass


MaybeSecure = str | SecureArg
CmdArgs = list[MaybeSecure]
