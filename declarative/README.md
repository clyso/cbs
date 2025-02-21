# Clyso Enterprise Storage

## RPMs

### Storing RPMs

RPMs will uploaded to [Clyso's S3](https://s3.clyso.com), and will live under
`ces-packages`.

Following Ceph upstream's hierarchy, RPMs will be organized as

```text
ces-packages/ceph/rpm-${CES_VERSION}/el${EL_VERSION}clyso/
```

Packages will thus be available in (for CES 25.03.1-ga.1)

```text
https://s3.clyso.com/ces-packages/ceph/rpm-25.03.1-ga.1/el9clyso/
```

Public key to verify RPM signatures will be available in

```text
https://s3.clyso.com/ces-packages/release.asc
```

### Installing RPMs

For simplicity, lets assume

```bash
RPM_URL=s3.clyso.com/ces-packages/ceph/rpm-25.03.1-ga.1/el9clyso
```

To install the RPMs referring to CES 25.03.1-ga.1, we will first install the
release RPM for this release.

This RPM will be located at

```text
https://${RPM_URL}/noarch/ceph-release-1-1.el9clyso.noarch.rpm
```

This package will install a repository file in `/etc/yum.repos.d/ces.repo`,
required to install this specific version of CES, and will contain the
following:

```text
[Ceph]
name=Clyso Ceph packages for $basearch
baseurl=https://s3.clyso.com/ces-packages/ceph/rpm-25.03.1-ga.1/el9clyso/$basearch
enabled=1
gpgcheck=1
type=rpm-md
gpgkey=https://s3.clyso.com/ces-packages/release.asc

[Ceph-noarch]
name=Clyso Ceph noarch packages
baseurl=https://s3.clyso.com/ces-packages/ceph/rpm-25.03.1-ga.1/el9clyso/noarch
enabled=1
gpgcheck=1
type=rpm-md
gpgkey=https://s3.clyso.com/ces-packages/release.asc

[ceph-source]
name=Clyso Ceph source packages
baseurl=https://s3.clyso.com/ces-packages/ceph/rpm-25.03.1-ga.1/el9clyso/SRPMS
enabled=1
gpgcheck=1
type=rpm-md
gpgkey=https://s3.clyso.com/ces-packages/release.asc
```

We will then be able to install all the required Ceph RPMs from these
repositories.
