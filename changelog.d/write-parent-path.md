### Fixed

- **A `write` path containing `..` is settled as the file it reaches.**
  The question and your permission rules see that file, following the path
  the way your system does. A `..` after a name that is not an existing
  directory is refused before anything is created.
