### Changed

- **A client with no terminal is told what kind of thing a requested call names.**
  The `tool_requested` progress frame carries `summary_kind`, one of `path`, `address`, `command` or `other`, so a client can tell a file from an address or a command without guessing from the words. The client protocol moves to revision 3, and a frame that says revision 2 is refused as an unsupported version.
