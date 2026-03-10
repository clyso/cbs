#!/bin/bash

set -ex &&
  dnf clean all &&
  rm -rf /var/cache/dnf/* &&
  rm -rf /var/lib/dnf/* &&
  rm -f /var/lib/rpm/__db* &&
  # remove unnecessary files with big impact
  rm -rf /etc/selinux /usr/share/selinux &&
  # don't keep compiled python binaries
  find / -xdev \( -name "*.pyc" -o -name "*.pyo" \) -delete &&
  rm -f /etc/yum.repos.d/{ceph,ganesha,tcmu-runner,ceph-iscsi}.repo
