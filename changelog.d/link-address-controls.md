### Security

- **A link's address can no longer carry terminal instructions.**
  The address of a link in an answer is now sent to the terminal in printable characters only, with anything else percent-escaped, so a model cannot end the link early and have the rest read as a command.
  Links still open where they pointed.
