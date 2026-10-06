### Security

- **On Unix, the new content of a file that `edit` or `write` replaces can be opened only by you until it takes the original's place.**
  It then has the original's mode, as before. The `crucible-workspace` test `a_replaced_files_preparation_file_is_owner_only_from_the_moment_it_is_made` holds this.
