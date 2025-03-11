#!/bin/bash

# remove once https://bugzilla.redhat.com/show_bug.cgi?id=2324959
# is addressed
ln -fs \
  /usr/lib/python3.9/site-packages/jaraco_text-4.0.0.dist-info \
  /usr/lib/python3.9/site-packages/jaraco.text-4.0.0.dist-info
