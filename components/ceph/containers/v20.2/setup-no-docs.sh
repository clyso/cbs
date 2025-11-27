#!/bin/bash

if grep -q 'tsflags' /etc/dnf/dnf.conf; then
  sed -i 's/tsflags=.*/tsflags=nodocs/g' /etc/dnf/dnf.conf
else
  echo "tsflags=nodocs" >>/etc/dnf/dnf.conf
fi
