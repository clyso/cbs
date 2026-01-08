#!/bin/bash

# CES - build Ceph RPMs
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.

build_ceph_rpms() {
  local ceph_dir="${1}"
  local dist_version=".el${2}.clyso"
  local topdir="${3:-${HOME}/rpmbuild}"
  local version="${4}"

  ccache=
  if [[ -n ${CES_CCACHE_PATH} ]]; then
    echo "CES build rpms with CCACHE: ${CES_CCACHE_PATH}"
    ccache="-DWITH_CCACHE=ON"
    CCACHE_DIR="${CES_CCACHE_PATH}"
    export CCACHE_DIR

    CEPH_EXTRA_CMAKE_ARGS="-DWITH_CCACHE=ON"
    export CEPH_EXTRA_CMAKE_ARGS
  fi

  for i in BUILD SOURCES RPMS SRPMS SPECS; do
    mkdir -p "${topdir}/${i}" || true
  done

  echo "Build Ceph SRPMs and RPMs"

  pushd "${ceph_dir}" >/dev/null || exit 1
  git submodule sync --recursive || exit 1
  git submodule update --init --recursive || exit 1
  ./do_cmake.sh -DCMAKE_BUILD_TYPE=RelWithDebInfo ${ccache} || exit 1
  ./make-dist "${version}" || exit 1

  echo "move ceph tarball to ${topdir}/SOURCES/"
  mv "ceph-${version}.tar.bz2" "${topdir}"/SOURCES/ || exit 1

  echo "building srpms"
  rpmbuild \
    --without=crimson \
    --define "_topdir ${topdir}" \
    --define "dist ${dist_version}" \
    -bs ceph.spec || exit 1

  echo "building rpms"
  rpmbuild \
    --without=crimson \
    --define "_topdir ${topdir}" \
    --define "dist ${dist_version}" \
    -rb "${topdir}"/SRPMS/*.src.rpm || exit 1

  popd >/dev/null || exit 1
}

# This function was initially copied, and then heavily based, on the function
# of the same name from 'ceph/ceph-build', in 'ceph-rpm-release/build/build'.
# It has been significantly modified for CES purposes instead of upstream's.
build_ceph_release_rpm() {
  local el_version="${2}"
  local topdir="${3}"
  local version="${4}"

  echo "Build Ceph RPM release package"

  summary="CES Ceph repository configuration"
  project_url=https://www.clyso.com/
  epoch=1 # means a non-development release (0 would be development)
  base_url="https://s3.clyso.com/ces-packages"
  target="el${el_version}.clyso"
  repo_base_url="${base_url}/components/ceph/rpm-${version}/${target}"
  # repo_base_url="http://download.ceph.com/rpm-${ceph_release}/${target}"
  gpgcheck=1
  gpgkey=https://s3.clyso.com/ces-packages/release.asc
  dist_version=".el${el_version}.clyso"

  cat <<EOF >"${topdir}"/SPECS/ceph-release.spec
Name:           ceph-release
Version:        2
Release:        ${epoch}%{?dist}
Summary:        ${summary}
Group:          System Environment/Base
License:        GPLv2
URL:            ${project_url}
Source0:        ceph.repo
BuildRoot:      %{_tmppath}/%{name}-%{version}-%{release}-root-%(%{__id_u} -n)
BuildArch:      noarch

%description
This package contains the Clyso Enterprise Storage's Ceph repository's
configuration for yum and up2date.

%prep

%setup -q  -c -T
install -pm 644 %{SOURCE0} .

%build

%install
rm -rf %{buildroot}
install -dm 755 %{buildroot}/%{_sysconfdir}/yum.repos.d
install -pm 644 %{SOURCE0} \
    %{buildroot}/%{_sysconfdir}/yum.repos.d

%clean

%post

%postun

%files
%defattr(-,root,root,-)
/etc/yum.repos.d/*

%changelog
* Sat Mar 1 2025 Joao Eduardo Luis <joao@clyso.com> 2-1
- Adjust for Clyso Enterprise Storage packages
* Fri Aug 12 2016 Alfredo Deza <adeza@redhat.com> 1-1
* Mon Jan 12 2015 Travis Rhoden <trhoden@redhat.com> 1-1
- Make .repo files be %config(noreplace)
* Sun Mar 10 2013 Gary Lowell <glowell@inktank.com> - 1-0
- Handle both yum and zypper
- Use URL to ceph git repo for key
- remove config attribute from repo file
* Mon Aug 27 2012 Gary Lowell <glowell@inktank.com> - 1-0
- Initial Package
EOF
  #  End of ceph-release.spec file.

  # Install ceph.repo file
  cat <<EOF >"${topdir}"/SOURCES/ceph.repo
[Ceph]
name=Clyso Ceph packages for \$basearch
baseurl=${repo_base_url}/\$basearch
enabled=1
gpgcheck=${gpgcheck}
type=rpm-md
gpgkey=${gpgkey}

[Ceph-noarch]
name=Clyso Ceph noarch packages
baseurl=${repo_base_url}/noarch
enabled=1
gpgcheck=${gpgcheck}
type=rpm-md
gpgkey=${gpgkey}

[ceph-source]
name=Clyso Ceph source packages
baseurl=${repo_base_url}/SRPMS
enabled=1
gpgcheck=${gpgcheck}
type=rpm-md
gpgkey=${gpgkey}
EOF
  # End of ceph.repo file

  # build source packages for official releases
  rpmbuild -ba \
    --define "_topdir ${topdir}" \
    --define "_unpackaged_files_terminate_build 0" \
    --define "dist ${dist_version}" \
    "${topdir}"/SPECS/ceph-release.spec
}

build_ceph_rpms "$@" || exit 1
build_ceph_release_rpm "$@" || exit 1
