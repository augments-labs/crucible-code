### Security

- **Spaces inside a quoted argument are part of the command a rule or an approval is about.**
  `rm -f "report final.txt"` and `rm -f "report  final.txt"` name two files, and they are now two commands to an allow rule and to a `session` answer.
  Runs of spaces between words still collapse, so `cargo   test` is still `cargo test`.
