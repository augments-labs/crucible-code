### Internal

- **crucible can now check an installer-managed release against its receipt.**
  It finds the install from its own executable, requires each directory and file there to belong to root or to the user running it and to be writable by nobody else, and refuses a link where none belongs, a receipt that breaks its format and a file whose SHA-256 is not the recorded one. Nothing acts on the result yet.
