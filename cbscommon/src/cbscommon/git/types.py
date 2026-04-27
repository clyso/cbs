# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Clyso GmbH


import re
from enum import Enum
from typing import final

SHA = str


class PushInfoLineStatus(Enum):
    UPDATED = 1
    REJECTED = 2
    OTHER = 3


@final
class PushInfoLine:
    _NEW_HEAD = "*"
    _FAST_FORWARD = " "
    _ERROR = "!"

    def __init__(self, line: str) -> None:
        self.status = PushInfoLineStatus.OTHER
        # each line is in form: <flag> \t <from>:<to> \t <summary> (<reason>)
        flag, from_to, summary = line.split("\t", 2)
        self.flag = flag

        # from git_utils only if new head or fast forward than its status is updated
        if flag == self._NEW_HEAD or flag == self._FAST_FORWARD:
            self.status = PushInfoLineStatus.UPDATED

        # change status to rejected if status flag is "!"
        # or if summary contains [no match] see GitPython
        if flag == self._ERROR:
            self.status = PushInfoLineStatus.REJECTED
        if summary.startswith("[") and "[no match]" in summary:
            self.status = PushInfoLineStatus.REJECTED

        _, to_ref = from_to.split(":", 1)
        # remove /refs/heads/ or /refs/tags/ from the beginning
        self.remote_ref_name = re.sub("^/refs/(heads|tags)/", "", to_ref)
