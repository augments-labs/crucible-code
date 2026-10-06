### Fixed

- **A link to a file in an answer opens the file, at its line, in any terminal that opens links.**
  A relative path such as `src/main.rs:12` is now handed to the terminal as an absolute `file://` address in the checkout, which VS Code, JetBrains IDEs, kitty and the desktop opener can each open.
  Before, a path was handed on as written, which most terminals could not open.
