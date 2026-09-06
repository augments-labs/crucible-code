//! No mediator can be constructed on a backend without domain enforcement.
//!
//! The shared process owner retains an optional mediator. An uninhabited type
//! preserves that cleanup interface without compiling an unused proxy or
//! providing a constructor that could bypass native capability negotiation.

use std::io;

use crucible_core::SandboxOutput;

#[derive(Debug)]
pub(super) enum Mediator {}

impl Mediator {
    pub(super) fn stop(&mut self) -> io::Result<()> {
        match *self {}
    }

    pub(super) fn protect_output(&self, _output: Box<dyn SandboxOutput>) -> Box<dyn SandboxOutput> {
        match *self {}
    }
}
