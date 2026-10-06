### Fixed

- **A sandboxed command's last output is no longer lost when it ends.**
  On Linux, output still in the pipe when a command finished could be cut off and read as if it were the whole output, so a tool could tell the model it had everything. The sandbox now gives the reader a moment to take all of it, and output it still has to cut is reported as incomplete.
