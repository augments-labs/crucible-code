### Fixed

- **A whole number written as `6.0` in a configuration file loads.** The
  published schema already accepted it, so an editor could pass a file that
  crucible then refused to start with. It is read as `6` and held to the same
  bounds, and `6.5` is still refused.
