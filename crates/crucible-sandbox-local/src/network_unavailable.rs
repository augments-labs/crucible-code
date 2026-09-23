//! No mediator can be constructed on a backend without domain enforcement.
//!
//! The shared process owner retains an optional mediator. An uninhabited type
//! preserves that cleanup interface without compiling an unused proxy or
//! providing a constructor that could bypass native capability negotiation.

use std::io;

#[derive(Debug)]
pub(super) enum Mediator {}

impl Mediator {
    pub(super) fn stop(&mut self) -> io::Result<()> {
        match *self {}
    }

    pub(super) fn masked(&self) -> Vec<Vec<u8>> {
        match *self {}
    }
}
