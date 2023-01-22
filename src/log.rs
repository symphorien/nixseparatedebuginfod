// SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
//
// SPDX-License-Identifier: GPL-3.0-only

use std::fmt::Display;

pub trait ResultExt {
    fn or_warn(self);
}

impl<T: Display> ResultExt for Result<(), T> {
    fn or_warn(self) {
        match self {
            Ok(()) => (),
            Err(e) => tracing::warn!("{:#}", e),
        }
    }
}
