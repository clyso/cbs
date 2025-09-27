#!/bin/bash

el_version="${1}"

: "${el_version:?}"

echo "noarch/ceph-release-2-1.el${el_version}.clyso.noarch.rpm"
