### Added

- **Ctrl+Enter and Ctrl+S stop the running turn and send the queue as the next one.**
  Ctrl+Enter stops the running turn and sends every queued prompt, then the line in the box, as the next turn; Ctrl+S stops it and sends the highlighted prompt alone, and the rest follow once it ends.
  VTE terminals such as GNOME Terminal, macOS Terminal and tmux without `extended-keys` send Ctrl+Enter as Enter; Ctrl+S works everywhere.
