### Fixed

- **When the system Bubblewrap cannot report its version, the sandbox says why.**
  The reason names the exit status of `bwrap --version` and what it printed, cut to 200 bytes, instead of only calling the version invalid.
