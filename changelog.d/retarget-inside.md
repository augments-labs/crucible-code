### Security

- **`read`, `edit`, `write`, `grep` and `glob` act only on the file you approved, inside the workspace as well as outside it.**
  If a path leads to a different file by the time the call runs, the call is refused before anything is read, created or changed.
  The tests named `retargeted` in `crucible-builtins` hold this.
