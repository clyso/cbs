// Copyright (C) 2026  Clyso
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Config(String),
    Connection(String),
    Api { status: u16, message: String },
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Config(msg) => write!(f, "config error: {msg}"),
            Error::Connection(msg) => write!(f, "connection error: {msg}"),
            Error::Api { status, message } => write!(f, "server error ({status}): {message}"),
            Error::Other(msg) => write!(f, "error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
