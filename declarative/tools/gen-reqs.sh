#!/bin/bash

# CBS - Generate Requirements
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

echo "generate ssl certs..."
openssl req -x509 -newkey rsa:4096 -nodes \
	-out ./cbs-cert.pem -keyout ./cbs-key.pem -days 365 || exit 1
echo "certs in 'cbs-cert.pem' and 'cbs-key.pem'"

echo "generate keys..."
server_key=$(openssl rand -hex 32)
token_key=$(openssl rand -hex 32)

echo "server key: ${server_key}"
echo " token key: ${token_key}"

echo ""
echo "please adjust the config file to reflect these values"
