### Security

- **A command allowed for the session covers only the exact line it was asked about.**
  The same commands joined by another operator are asked about again, while allow rules still cover each command in a line as before.
  The `remembered` tests in `crucible-tools`, such as `a_remembered_command_does_not_cover_its_commands_run_regardless_of_each_other`, hold this.
