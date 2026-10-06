# Environment variables

Every variable a shipped crucible reads, and what it does with each, grouped
the way you are likely to look for them. Each entry says what the variable
does and which page owns the behaviour, and where unset matters, what happens
without it.

Two rules hold across the list. A variable crucible reads to configure itself
begins with `CRUCIBLE_CODE_`, and nothing else it reads does: every other name
here belongs to a vendor or to the operating system, and crucible reads it
without deciding what it means. And crucible never writes to its own
environment; the one exception is a library it opens the clipboard with, noted
in that entry. What it sets, it sets for the processes it starts, which the
last two sections cover, plus one internal setting it hands its own Windows
sandbox helper.

Two more are read only if crucible crashes: Rust's own `RUST_BACKTRACE` and
`RUST_LIB_BACKTRACE` decide whether the crash message carries a backtrace.

## crucible's own settings

- `CRUCIBLE_CODE_HOME`: where crucible keeps its files: `config.json`,
  `auth.json`, the cached release check, the `sessions` directory and the
  `extensions` directory. It is taken as the directory itself, not as somewhere
  to put a `.crucible` inside, and only when it is an absolute path; a relative
  one is ignored. Unset:
  `.crucible` under your home directory. It is read to find the configuration
  file, so it is the one setting of crucible's own that no configuration file
  can carry: a file that sets it is refused with `env cannot set
  CRUCIBLE_CODE_HOME`. See
  [`CRUCIBLE_CODE_HOME`](../configuration/configuration.md#crucible_code_home).
- `CRUCIBLE_CODE_MOUSE_SCROLL_SPEED`: how many rows of the transcript one notch
  of the wheel moves, a whole number from `3` to `30`. Unset: `6`. It can also
  be written in the [`env`](../configuration/configuration.md#env) block of a
  configuration file, as an integer or a string, and the shell you start crucible in wins over the block.
  A value crucible cannot read is refused rather than rounded into range: in a
  file with `is not set to an answer crucible takes`, in the shell with
  `CRUCIBLE_CODE_MOUSE_SCROLL_SPEED is not set to an answer crucible takes`,
  each followed by what it does take, `a whole number of rows from 3 to 30`.
  See
  [`CRUCIBLE_CODE_MOUSE_SCROLL_SPEED`](../configuration/configuration.md#crucible_code_mouse_scroll_speed).

The `env` block is the environment the commands crucible runs are started
with, and crucible's own settings are read from it under this prefix. A
configuration file under the working directory may set only names that begin
with `CRUCIBLE_CODE_`; any other name there is refused with `env cannot set`
and the name, and the message ends: `Only crucible's own settings, which start
with CRUCIBLE_CODE_, are read from one. Put this in the configuration file in
your home directory, or set it in the shell you start crucible in`. See
[the workspace files](../configuration/configuration.md#the-workspace-files).

## Where crucible keeps its files

- `HOME`: your home directory. crucible's own directory is `.crucible` under
  it, unless `CRUCIBLE_CODE_HOME` moves it. Only an absolute path counts.
  With neither variable naming a directory, crucible stops before reading any
  file: `crucible has nowhere to keep its files: set HOME, or set
  CRUCIBLE_CODE_HOME to the absolute path of the directory you want it to
  use`. See [the files](../configuration/configuration.md#the-files).
- `USERPROFILE`: on Windows only, asked when `HOME` is unset or is not an
  absolute path. A `HOME` inherited from Git Bash can arrive as `/c/Users/ada`,
  which Windows does not call absolute, so `USERPROFILE` answers instead and
  both shells land in the same directory.
- `XDG_DATA_HOME`: read only to find session logs written by crucible 0.0.2 or
  earlier. When `sessions` under crucible's directory is not a directory and
  `$XDG_DATA_HOME/crucible/sessions` is (or, with `XDG_DATA_HOME` unset or not
  an absolute path, `$HOME/.local/share/crucible/sessions`, where `HOME` is
  again the absolute one and never `USERPROFILE`), sessions stay there and are
  read in place; nothing is copied or moved. With `CRUCIBLE_CODE_HOME` set,
  the older tree is not looked for. See
  [if you used crucible 0.0.2 or earlier](../sessions/sessions.md#if-you-used-crucible-002-or-earlier).

## Searching the tree

- `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `XDG_CONFIG_HOME`: read when `grep`
  or `glob` walks the tree, by the library that walks it, to find your global
  git excludes list. The list is the file `core.excludesFile` names in the
  first of these that sets it: the file `GIT_CONFIG_GLOBAL` names, then
  `.gitconfig` in your home directory, then `git/config` under
  `XDG_CONFIG_HOME` (or under `.config` in your home directory), then the file
  `GIT_CONFIG_SYSTEM` names, or `/etc/gitconfig`. With none, it is `git/ignore`
  under `XDG_CONFIG_HOME`, or `.config/git/ignore` in your home directory. A
  `~` in the setting stands for your home directory. Any of the three set to
  nothing counts as unset, and your home directory here is `HOME` on Unix and
  `USERPROFILE` on Windows, or the account's own when that is unset or empty.
  See [what both of them skip](../tools/searching.md#what-both-of-them-skip).

## Provider keys

- `ANTHROPIC_API_KEY`, `DASHSCOPE_API_KEY`, `DEEPSEEK_API_KEY`,
  `GEMINI_API_KEY`, `META_API_KEY`, `MIMO_API_KEY`, `MINIMAX_API_KEY`,
  `MOONSHOT_API_KEY`, `OPENAI_API_KEY`, `XAI_API_KEY`, `ZAI_API_KEY`: the key
  for `anthropic`, `qwen`, `deepseek`, `google`, `meta`, `mimo`, `minimax`,
  `moonshot`, `openai`, `xai` and `zai` respectively, unless `providers.<name>.apiKeyEnv`
  names another variable to read instead. The value is trimmed, and unset or
  blank is treated as no key, which is how a shell turns a variable off for
  one run. A key in `DASHSCOPE_API_KEY`, `MINIMAX_API_KEY` or `ZAI_API_KEY`
  is sent to the vendor's international site; a key of its mainland China site
  is given through `/login`, or reaches it with `providers.<name>.baseUrl`.
  What a provider is used with is settled in this order: an account
  login stored for it (where the provider supports one and no `baseUrl` is
  set), then the variable, then a key stored in `auth.json`. See
  [keys](../providers/providers.md#keys) and
  [a key written down instead of exported](../providers/providers.md#a-key-written-down-instead-of-exported).
- `envFrom` in an `mcp.servers` record names variables in crucible's own
  environment whose values are handed to that server as credentials. One that
  is not set stops the server from starting: `envFrom names NAME, which is not
  set in crucible's own environment`. The record's `env` values go as written,
  and the server is started with those two blocks and nothing else. See
  [MCP servers](../configuration/configuration.md#mcp-servers).

## Proxy and TLS

- `ALL_PROXY`, `all_proxy`, `HTTPS_PROXY`, `https_proxy`, `HTTP_PROXY`,
  `http_proxy`: tried in that order, and the first that holds an address
  crucible can read is the proxy for every request, whatever the address's
  scheme. No scheme means `http`. A `socks://`, `socks4://` or `socks5://`
  address is taken and then connected past, straight to the host; a
  `socks4a://` or `socks5h://` one refuses every request to a host `NO_PROXY`
  does not name. Unset: crucible connects directly. These are read once, from
  the environment crucible was started in; the `env` block reaches the commands crucible runs and not
  crucible's own requests, and no proxy setting of the operating system is
  read. A value that is not valid Unicode counts as unset. See
  [through a proxy](../providers/network.md#through-a-proxy).
- `NO_PROXY`, `no_proxy`: hosts reached directly. The first that is set is
  used, even when set to nothing, so `NO_PROXY=` hides `no_proxy`. Entries are
  separated by commas and not trimmed. See
  [hosts that skip the proxy](../providers/network.md#hosts-that-skip-the-proxy).
- Certificates: no variable. crucible trusts the Mozilla roots built into it,
  over TLS 1.2 or 1.3, and reads neither `SSL_CERT_FILE`, `SSL_CERT_DIR` nor
  the operating system's store. See
  [certificates](../providers/network.md#certificates).

## Terminal and colour

All of these are read at start, and configuration overrides some of them.
`output.color` decides whether colour is written at all: `always` writes it
on a terminal despite `NO_COLOR`, and `never` writes none. An output that is
not a terminal gets no colour whichever is set. Neither raises what `TERM`
allows, so `always` with `TERM` at `dumb` or unset still writes no colour
unless `COLORTERM` says `truecolor` or `24bit`. `output.theme` set to anything
but `auto` decides which table is drawn whatever `COLORFGBG` says. No key in
[configuration](../configuration/configuration.md#output) overrides
`COLORTERM`, `TERM`, or the four that stop the terminal being asked.

- `NO_COLOR`: set to anything but empty, with `color` at `auto` (the default),
  turns colour off. `always` and `never` decide without reading it. It does
  not affect links, which are written whenever a terminal is attached and
  `TERM` is not `dumb`.
- `TERM`: `dumb` turns colour off and stops crucible writing links. A value
  containing `256color` gives the indexed table of 256 colours; any other
  value gives the sixteen basic colours. Unset turns colour off, because
  whatever is reading is not saying it is a terminal.
- `TERMINAL_EMULATOR`: `JetBrains-JediTerm`, which a JetBrains IDE sets in
  its terminal, writes the line of a link to a file as `:12` after the path,
  the form that terminal reads. Any other value, or none, writes it as
  `#12`, which VS Code and kitty read and the desktop's file opener drops.
- `COLORTERM`: `truecolor` or `24bit` gives exact colours, whatever `TERM`
  says. Any other value is ignored.
- `COLORFGBG`: `fg;bg`, or `fg;other;bg`, the rxvt convention. The last field
  is the background: `0` to `6` and `8` mean dark, `7` and `9` to `15` mean
  light, anything else says nothing. With `theme` at `auto`, it seeds which
  table crucible draws with; a background the terminal reports when asked
  outranks it, and with neither, `auto` is dark. Unset: the terminal is asked,
  or dark.
- `SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY`, `TMUX`: any of these set to
  something non-empty, or `TERM` starting with `screen`, says the terminal is
  somewhere else or behind a multiplexer, where asking it for its background
  would be slow. crucible then does not ask, and `COLORFGBG` or the dark table
  stands in. The terminal is asked only when colour will be written and both
  input and output are a terminal.

The argument parser draws its own colour, for `--help`, `--version` and a
mistyped flag, before any configuration is read, and decides it from a
separate set of rules. `NO_COLOR` set to anything but empty turns it off.
Otherwise `CLICOLOR_FORCE` set to anything but empty turns it on even down a
pipe, and `CLICOLOR` at `0` turns it off. On a terminal it is drawn when
`TERM` allows colour, or when `CLICOLOR` is set to anything else, or when `CI`
is set at all. On Windows an unset `TERM` allows colour too; anywhere,
`TERM=dumb` does not. `output.color` has no say here.

## The clipboard

- `WAYLAND_DISPLAY`, `WAYLAND_SOCKET`, `XDG_RUNTIME_DIR`, `DISPLAY`,
  `XAUTHORITY`: on Linux, read when you press Ctrl+V to paste an image, to
  open the desktop clipboard. With `WAYLAND_DISPLAY` set, the Wayland
  clipboard is tried first: `WAYLAND_SOCKET` is the file descriptor number of
  a connection already open, and without it `WAYLAND_DISPLAY` is the socket's
  path, absolute as it stands or a name under `XDG_RUNTIME_DIR`, which has to
  be absolute. The library removes `WAYLAND_SOCKET` from crucible's
  environment once it has read it. Without `WAYLAND_DISPLAY`, or when the
  Wayland clipboard cannot be opened, the X11 clipboard is tried: `DISPLAY`
  names the server, and `XAUTHORITY` names the authority file, or
  `.Xauthority` under `HOME`. When neither opens, the paste is refused with
  `the clipboard could not be opened` and the library's reason, written under
  the box. It is opened at the first paste and kept for the later ones. macOS
  and Windows read no variable for this. See
  [run it](../getting-started/getting-started.md#run-it).

## What a command is started with

A command run by the `bash` tool is not started with the environment crucible
was started in. It gets the variables below, read once when crucible starts,
with whatever the [`env`](../configuration/configuration.md#env) block adds. A
name in the block replaces the inherited one, and an inherited name that is
unset stays unset. Nothing else crosses, your provider key included. See
[what a command is started with](../tools/commands.md#what-a-command-is-started-with).

- On Unix: `PATH`, `HOME`, `LC_ALL`, `LC_CTYPE`, `LANG`, `TERM`, `TMPDIR`.
- On Windows: `PATH`, `PATHEXT`, `COMSPEC`, `SystemRoot`, `SystemDrive`,
  `windir`, `TEMP`, `TMP`, `HOME`, `USERPROFILE`, `HOMEDRIVE`, `HOMEPATH`,
  `APPDATA`, `LOCALAPPDATA`, `ProgramFiles`, `ProgramFiles(x86)`,
  `ProgramData`, `TERM`. A name in the `env` block replaces an inherited one
  only where the two are spelled with the same case: a block's `PATH` beside
  an inherited `Path` leaves both in the map. Unconfined, the block's one is
  what the command sees; a confined command is not started, with `Windows
  sandbox environment is ambiguous`.

Some of the same variables are read by crucible itself, to find a program:

- `PATH`: where the shell is looked for, in absolute directories only; a
  relative entry is never looked in. On Unix, `sh` on the `PATH`, then
  `/bin/sh`, then `/usr/bin/sh`. On Windows, `sh.exe` on the `PATH`, then
  `usr\bin\sh.exe` under `Git` in `ProgramFiles`, `ProgramFiles(x86)` and
  `LocalAppData\Programs`. Without one, every command fails with `no POSIX
  shell to run it with`, and the reason given is `no sh on the PATH or in the
  places one lives` on Unix and `install Git for Windows, which carries one,
  or put an sh.exe on the PATH` on Windows. On Linux, `PATH` is also where a
  system Bubblewrap is looked for when confinement is turned on, and its
  absence is reported as `PATH is unavailable while discovering system
  Bubblewrap`. See
  [operating-system confinement](../security/sandboxing.md#turning-it-on).
- `PATH`, and on Windows `ProgramFiles`, `ProgramFiles(x86)`, `LOCALAPPDATA`,
  `ProgramData` and `USERPROFILE`: when `read` meets a document, spreadsheet
  or video it cannot read as text, it names a converter to run. `soffice`,
  `ffmpeg` or `ffprobe` is named bare when the `PATH` finds it, and by its
  absolute path when it sits where an installer or package manager put it
  instead: `LibreOffice\program` under `ProgramFiles` or `ProgramFiles(x86)`,
  and the WinGet, Chocolatey and Scoop shim directories under `LOCALAPPDATA`,
  `ProgramFiles`, `ProgramData` and `USERPROFILE`. On macOS the places looked
  in are fixed directories; on Linux only the `PATH` is searched. See
  [a document is read by converting it first](../tools/files.md#a-document-is-read-by-converting-it-first).
- `PATH`: also where an MCP server's `command` is looked for when it is not
  written as an absolute path, as `NAME` (`NAME.exe` on Windows) in the same
  absolute directories. One not found stops the server: `no NAME on the PATH`,
  with the command as written in place of `NAME`. See
  [MCP servers](../configuration/configuration.md#mcp-servers).

## What confinement sets or clears

A confined command receives the map above through a cleared process
environment, with these changes made by the platform's backend. See
[environment and credentials](../security/sandboxing.md#environment-and-credentials).

- Linux: `HOME` and `TMPDIR` are dropped from the map, and `SSH_AUTH_SOCK` and
  `GPG_AGENT_INFO` are removed even if the `env` block names them; then `HOME`
  is set to `/crucible-home` and `TMPDIR` to `/tmp`, both private to the
  command.
- macOS: `TMPDIR` is set to a private scratch directory that is removed with
  the command. It is made under the directory `TMPDIR` names in crucible's own
  environment, or with that unset, the per-user temporary directory macOS
  reports, or `/tmp`.
- Windows: `TEMP` and `TMP`, however they are capitalised, are replaced with
  one private directory that is removed with the command. It is made under the
  temporary directory Windows reports for the process.
  The helper that starts the command, `crucible-sandbox-broker.exe`, is
  itself started through a cleared environment holding only `SystemRoot`,
  kept because Windows resolves system components through it; the command's
  map reaches it on its standard input instead.
- A command under a network domains policy is given `HTTP_PROXY`,
  `HTTPS_PROXY`, `ALL_PROXY`, `http_proxy`, `https_proxy` and `all_proxy`, all
  set to crucible's own per-command proxy as `http://name:secret@address`,
  and `NO_PROXY` and `no_proxy` set to nothing. These replace whatever `env`
  set, bypass lists included. Linux sets them for any domains policy; macOS
  only where the policy allows at least one domain. Windows has no per-command
  proxy and sets none of them. See
  [commands connect on their own](../providers/network.md#commands-connect-on-their-own).

Unconfined execution also starts from a cleared environment and hands the
command the same map, unchanged. See
[unconfined execution](../security/sandboxing.md#unconfined-execution). An MCP
server is started the same way, with its record's `env` and `envFrom` as the
whole of its map.

The program `/login` opens your browser with (`xdg-open` on Linux, `open` on
macOS, `explorer.exe` on Windows) is the one child started with crucible's own
environment, because a browser needs your display and desktop session to open.
It is started without the variables a provider key is read from: each
provider's usual one, such as `ANTHROPIC_API_KEY`, and any variable an
`apiKeyEnv` setting names. A variable an MCP server's `envFrom` names is not
withheld: the browser opener inherits it. It gets the address to open as its
only argument. See
[account login today](../providers/providers.md#account-login-today).
