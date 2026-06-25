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

//! Image descriptors and image tooling. C1 lands the external
//! image-descriptor lookup (`get_image_desc`) for `versions create`'s trailing
//! note (design 006); the skopeo/signing/sync tooling lands in M2 (design 008).

pub mod desc;
