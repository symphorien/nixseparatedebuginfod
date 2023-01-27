// SPDX-FileCopyrightText: 2023 Guillaume Girol <symphorien+git@xlumurb.eu>
//
// SPDX-License-Identifier: GPL-3.0-only


//! Logging utilities

use std::fmt::Display;

/// Adds a way to log errors to [Result]
pub trait ResultExt {
    /// if `self` is an error, then calls [tracing::warn!] with this error
    ///
    /// otherwise does nothing
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
