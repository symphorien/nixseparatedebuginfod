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
