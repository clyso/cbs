# CBS service library - routes - utilities
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

from fastapi import Depends, HTTPException, status

from cbslib.builds.mgr import BuildsMgr
from cbslib.core.mgr import CBSMgr


def get_builds_mgr(mgr: CBSMgr) -> BuildsMgr:
    builds_mgr = mgr.builds_mgr
    if not builds_mgr.available:
        raise HTTPException(
            status_code=status.HTTP_503_SERVICE_UNAVAILABLE,
            detail="Service hasn't started yet, try again later.",
        ) from None
    return builds_mgr


CBSBuildsMgr = Annotated[BuildsMgr, Depends(get_builds_mgr)]
