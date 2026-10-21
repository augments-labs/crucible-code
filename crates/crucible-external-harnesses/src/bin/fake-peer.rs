//! A scripted stand-in for another vendor's agent; see
//! `crucible_external_harnesses::fake_peer`.

fn main() -> std::process::ExitCode {
    crucible_external_harnesses::fake_peer::entry()
}
