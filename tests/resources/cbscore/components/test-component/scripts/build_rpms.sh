#!/bin/bash

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

build_test_comp_rpms() {
  local test_comp="${1}"
  local dist_version=".el${2}.clyso"
  local topdir="${3:-${HOME}/rpmbuild}"
  local version="${4}"

  for i in BUILD SOURCES RPMS SRPMS SPECS; do
    mkdir -p "${topdir}/${i}" || true
  done


  echo "Build test_comp SRPMs and RPMs"

  cp $test_comp/test-component.sh ${topdir}/SOURCES/test-component.sh
  
  cat <<EOF >"${topdir}"/SPECS/test-component.spec
Name:           test-component
Version:        ${version}
Release:        1%{?dist}
Summary:        A simple bash script
License:        GPL
BuildArch:      noarch

Source0:        test-component.sh

%description
This package installs a simple hello world.

%prep
# No tarball to unpack, so we just copy the sources to the BUILD directory
%setup -q -c -T
cp %{SOURCE0} .

%install
# 1. Install the script to /usr/bin
mkdir -p %{buildroot}%{_bindir}
install -m 755 test-component.sh %{buildroot}%{_bindir}/test-component

%files
%{_bindir}/test-component

%changelog
* Mon Feb 23 2026 Your Name <you@example.com> - 1.0.0-1
- Initial build of the hello world tool
EOF

  echo "building"
  rpmbuild \
    --define "_topdir ${topdir}" \
    --define "dist ${dist_version}" \
    -bb "${topdir}"/SPECS/test-component.spec || exit 1
}

# This function was initially copied, and then heavily based, on the function
# of the same name from 'ceph/ceph-build', in 'ceph-rpm-release/build/build'.
# It has been significantly modified for CES purposes instead of upstream's.
build_test_comp_release_rpm() {
  local el_version="${2}"
  local topdir="${3}"
  local version="${4}"
  local base_url=$"${5:-https://s3.test-corp.com/test_comp-packages}"
  local gpgcheck=$"${6:-1}"

  echo "Build test_comp RPM release package"

  summary="test-component repository configuration"
  project_url=https://www.clyso.com/
  epoch=1 # means a non-development release (0 would be development)
  target="el${el_version}.clyso"
  repo_base_url="${base_url}/components/test-component/rpm-${version}/${target}"
  gpgkey=https://s3.test_comp.com/test_comp-packages/release.asc
  dist_version=".el${el_version}.clyso"

  cat <<EOF >"${topdir}"/SPECS/test-component-release.spec
Name:           test-component-release
Version:        2
Release:        test
Summary:        summary
Group:          System Environment/Base
License:        GPLv2
URL:            http://test-corp.org
Source0:        test-component-release.repo
BuildRoot:      %{_tmppath}/%{name}-%{version}-%{release}-root-%(%{__id_u} -n)
BuildArch:      noarch

%description
This package contains the test-component-release repository's
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
* Mon Feb 23 2026 Your Name <you@example.com> - 1.0.0-1
- Initial Package
EOF
  #  End of ceph-release.spec file.

  # Install ceph.repo file
  cat <<EOF >"${topdir}"/SOURCES/test-component-release.repo
[test-component-noarch]
name=test-component noarch packages
baseurl=${repo_base_url}/noarch
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
    "${topdir}"/SPECS/test-component-release.spec
}

build_test_comp_rpms "$@" || exit 1
build_test_comp_release_rpm "$@" || exit 1
