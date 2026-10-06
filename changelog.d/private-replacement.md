### Security

- **A file being replaced is unreadable by other accounts until the replacement is finished.**
  On Unix, `edit` and a `write` over an existing file prepare the new content in a file only you can open, and give it the original's mode just before it takes the original's place.
