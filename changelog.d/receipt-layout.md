### Internal

- **crucible can now check an installer-managed release against its receipt.**
  It finds the install from its own executable, holds every directory and file there to the owner and mode the sandbox broker is held to, and refuses a link where none belongs, a receipt that breaks its format and a file whose SHA-256 is not the recorded one. Nothing acts on the result yet.
