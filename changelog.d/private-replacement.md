### Security

- **On Unix, the new content of a file that `edit` or `write` replaces can be opened only by you while it is being written.**
  It is given the original's mode just before it takes the original's place, as before. The `crucible-workspace` test `a_replaced_files_preparation_file_is_owner_only_from_the_moment_it_is_made` holds this.
