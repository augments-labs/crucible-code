# Keys

Every key and mouse action the terminal interface answers, grouped by where
you are when you press it. A key means one thing at the prompt and can mean
another inside a panel, so each section below is one place. The prose version
of most of this is in [Run it](../getting-started/getting-started.md#run-it);
this page is the list. Slash commands are not keys: they are listed under
[Commands](../getting-started/getting-started.md#commands).

## What holds everywhere

- A <kbd>Ctrl</kbd> key here is Control held on its own. With
  <kbd>Shift</kbd> added the letter is a different key, and one crucible gives
  no meaning to, so <kbd>Ctrl+Shift+C</kbd> stays your terminal's copy. Any
  other letter held with <kbd>Ctrl</kbd> or <kbd>Alt</kbd>, and the function
  keys, do nothing, apart from closing [the `/help`
  panel](#help-and-a-command-refused-mid-turn) shown while a turn runs.
- <kbd>Shift-Enter</kbd> opens a line only on a terminal that spells modified
  keys distinctly. Crucible asks for that spelling when it starts, a terminal
  without it discards the request, and there the press arrives as a plain
  <kbd>Enter</kbd>. <kbd>Alt-Enter</kbd> and <kbd>Ctrl+J</kbd> need nothing
  asked for.
- A mark in any list stops at each end. It never wraps round.
- The mouse is reported to crucible for the whole session. Hold
  <kbd>Shift</kbd> while you drag to hand the pointer back to your terminal's
  own selection.
- A click outside the rows of whatever is standing over the box is ignored.
  The wheel over a panel that is not a window over more text than it shows
  scrolls the transcript underneath it; over one that is, it walks the panel,
  and a notch the panel has no use for at either end goes to the transcript.
  The `/help` panel shown while a turn runs is the exception: a notch closes it.
- Resizing the window redraws whatever is standing.

## Writing a prompt

### Editing

| Key | What it does |
| --- | --- |
| <kbd>Enter</kbd> | Sends the prompt. Nothing while the box is empty. With the command list open, runs the marked command instead. |
| <kbd>Shift-Enter</kbd>, <kbd>Alt-Enter</kbd>, <kbd>Ctrl+J</kbd> | Opens a line under the cursor. `input.send` set to `altEnter` swaps this with <kbd>Enter</kbd>: see [`input`](../configuration/configuration.md#input). |
| <kbd>\\</kbd> and the sending key | A backslash just before the cursor makes the sending press open a line instead, and the backslash goes. |
| <kbd>Backspace</kbd>, <kbd>Delete</kbd> | Rubs out the character behind or ahead of the cursor. |
| <kbd>Ctrl+W</kbd>, <kbd>Ctrl+Backspace</kbd>, <kbd>Alt-Backspace</kbd> | Rubs out the word behind the cursor. |
| <kbd>Ctrl+U</kbd> | Rubs out from the cursor back to the start of its line. |
| <kbd>Ctrl+K</kbd> | Rubs out from the cursor to the end of its line. |
| <kbd>Tab</kbd> | Nothing. A tab inside pasted text becomes four spaces. |
| <kbd>Ctrl+Y</kbd> | Copies everything in the box exactly as typed, by asking your terminal to put it on the clipboard. The row under the box says `line copied` once the request is sent. Nothing while the box is empty. |
| <kbd>Ctrl+V</kbd> | Pastes an image from the clipboard and puts a marker such as `[Image #1]` in the box. When nothing readable is there, the reason is written under the box. |
| <kbd>Ctrl+C</kbd> | Clears the box, every line of it. When the box is already empty the row says `press ctrl+c again to leave`, and a second press within two seconds ends the session. |
| <kbd>Ctrl+D</kbd> | Ends the session, only while the box is empty. |

A line here is the text between two newlines, so on a line long enough to
have wrapped, <kbd>Ctrl+U</kbd>, <kbd>Ctrl+K</kbd>, <kbd>Home</kbd> and
<kbd>End</kbd> reach its real ends rather than the edges of the row. The keys
that clear, copy or end act on the whole box instead.

Text pasted from the terminal goes in whole, newlines included; every control
character but tab is left out. A paste of more than 1,000 characters is shown
in the box as a label such as `[Pasted text 1234 chars]` and sent in full. The
prompt holds up to 1 MiB, and past that the row under the box says `prompt is
limited to 1 MiB`.

The copy is a request written to the terminal (an OSC 52 request), so `line
copied` means it was asked for. Under a terminal or multiplexer that ignores
such requests the row still says `line copied`, and nothing reaches the
clipboard. A prompt over 64 KiB of text is not copied at all, nor is anything
when the output is not a terminal, and the row says `the line is too long for
the terminal to copy`.

### Moving

| Key | What it does |
| --- | --- |
| <kbd>←</kbd>, <kbd>→</kbd> | One character. |
| <kbd>Ctrl+←</kbd>, <kbd>Ctrl+→</kbd>, <kbd>Alt-←</kbd>, <kbd>Alt-→</kbd>, <kbd>Alt-B</kbd>, <kbd>Alt-F</kbd> | One word. A word is a run of anything that is not whitespace, so a path is one word. |
| <kbd>Home</kbd>, <kbd>End</kbd> | The start or the end of the cursor's line. |
| <kbd>↑</kbd>, <kbd>↓</kbd> | To the line above or below, keeping the column. A line ends at a newline, not at the edge of the window, so they do not move between the rows of a paragraph that has wrapped. On the first or last line they walk the command list if one is open, and the history otherwise, where <kbd>↓</kbd> needs a walk already open. |
| Click in the box | Puts the cursor where you pointed. |

### History

<kbd>↑</kbd> on the first line of the box, with no list open, walks back
through the prompts sent from this directory, newest first, and keeps what it
interrupted. The box need not be empty. <kbd>↓</kbd> walks forward again, and
one step past the newest puts that text back. The top border of the box says
where you are, such as `history 80/100`. Any edit ends the walk and leaves the
text yours; moving the cursor does not. Each directory keeps at most its last
hundred prompts between sessions, out of 512 kept across every directory, and a
blank prompt or one longer than 1024 bytes is never kept. [Run
it](../getting-started/getting-started.md#run-it) tells the longer story.

### The command list

While the box holds one word starting with `/`, the commands whose names begin
with it stand in a list above the box, and a bare `/` shows all of them.
<kbd>↑</kbd> and <kbd>↓</kbd> walk the list and <kbd>Enter</kbd> runs the
marked command. The list is not drawn where there is no room for the whole
of it, but it is still there: <kbd>↑</kbd> and <kbd>↓</kbd> still move its
unseen mark, and <kbd>Enter</kbd> runs the marked command rather than the
word as typed. The commands themselves are under
[Commands](../getting-started/getting-started.md#commands).

### Other keys at the prompt

| Key | What it does |
| --- | --- |
| <kbd>Shift-Tab</kbd> | Steps the permission mode on the press: `ask`, `allowEdits`, `fullAccess`, then `ask` again. See [Stepping it while you type](../permissions/modes.md#stepping-it-while-you-type). |
| <kbd>Ctrl+B</kbd> | Stands the list of commands left running. With none running it closes at once. A click on their count under the box does the same. |
| <kbd>Ctrl+O</kbd> | Stands the results the transcript cut short. Nothing while none were cut. |
| <kbd>Ctrl+T</kbd> | Expands the plan past its seven rows, or folds it back. Nothing without a plan. See [Seven rows, and the key that gives the rest back](../tools/planning.md#seven-rows-and-the-key-that-gives-the-rest-back). |
| <kbd>Esc</kbd>, <kbd>Ctrl+E</kbd>, <kbd>Ctrl+Q</kbd>, <kbd>Ctrl+R</kbd> | Nothing between turns. |
| Wheel | Scrolls the transcript. |

## While a turn runs

The box stays open while an answer streams in, and most keys mean what they
mean at the prompt: editing, moving, the history walk, the command list,
clicks, <kbd>Ctrl+Y</kbd>, <kbd>Ctrl+V</kbd>, <kbd>Ctrl+T</kbd> and the
wheel. These differ:

| Key | What it does |
| --- | --- |
| <kbd>Esc</kbd> | Asks the turn to stop. The row above the box reads `interrupting` until it has. A search or a walk stopped this way answers with what it found: see [Stopping one](../tools/searching.md#stopping-one). |
| <kbd>Enter</kbd> | Queues the prompt for the running turn. Up to 64 prompts and 1 MiB of text can wait; past either bound the prompt stays in the box and the row says `typed-ahead prompts are limited to 64 lines and 1 MiB`. A prompt that is a command is run or refused instead: see [A command typed while a turn runs](../getting-started/getting-started.md#a-command-typed-while-a-turn-runs). |
| <kbd>Shift-Tab</kbd> | Steps the mode for the turn that starts next, and the row under the box says which. The running turn keeps the mode it began under. |
| <kbd>Ctrl+C</kbd> | Clears the box. When it is already empty it offers to leave as at the prompt, and the second press within two seconds stops the turn and ends the session. |
| <kbd>Ctrl+D</kbd> | Nothing. |
| <kbd>Ctrl+B</kbd> | Leaves the running command in the background, when its row offers `(ctrl+b to background)`. Nothing otherwise. See [Leaving one running](../tools/commands.md#leaving-one-running). |
| <kbd>Ctrl+Q</kbd> | Stands the prompts waiting in the queue. Nothing while it is empty. |
| <kbd>Ctrl+O</kbd> | Stands the cut results under the tail of the answer, which goes on arriving above them. |
| <kbd>Ctrl+E</kbd>, <kbd>Tab</kbd> | Nothing. |

While the <kbd>Ctrl+O</kbd> view or the queue stands over a running turn it
has the keyboard: <kbd>Esc</kbd> closes it rather than stopping the turn. Its
keys are under [Results cut short](#results-cut-short) and [The
queue](#the-queue).

## Reading the conversation

| Action | What it does |
| --- | --- |
| Wheel | Scrolls the transcript, six rows a notch unless [`CRUCIBLE_CODE_MOUSE_SCROLL_SPEED`](../configuration/configuration.md#crucible_code_mouse_scroll_speed) says otherwise, from 3 to 30. Sending a prompt takes you back to the foot. |
| Pointer over a cut result | Lights it, every row of it. |
| Click on a cut result | Stands that one result, in the view <kbd>Ctrl+O</kbd> stands them all in. |
| Drag | Selects the rows you cover, anywhere in the window, and letting go copies them. At the top or the foot the transcript scrolls under the pointer, and the wheel scrolls it while the button is still down. Resizing lets go of the selection. |
| Click on `transcript map` | Opens the map along the bottom row. A click on it jumps there, and one on a prompt's mark lands on that prompt; a drag is exact; the wheel moves it. It closes three seconds after the last touch. |

[Moving through the transcript](../sessions/sessions.md#moving-through-the-transcript)
describes the map, and [Run it](../getting-started/getting-started.md#run-it)
the drag.

## Answering a permission question

A call that needs your verdict stands a panel where the box was:
[The question](../permissions/permissions.md#the-question).

| Key | What it does |
| --- | --- |
| <kbd>↑</kbd>, <kbd>↓</kbd> | Moves the mark. While the explanation is open and was cut to fit, scrolls it instead. |
| <kbd>1</kbd>, <kbd>2</kbd>, <kbd>3</kbd> | Takes that answer at once: `Yes, once`, `Yes, and don't ask again this session`, `No, and end the turn`. |
| <kbd>Enter</kbd> | Takes the marked answer. |
| <kbd>Ctrl+E</kbd> | Opens the explanation the call sent, under the command or the path, and hides it again. Only where the footer names it. |
| <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd>, <kbd>Ctrl+D</kbd> | Refuses, the same as `No, and end the turn`. |
| Anything else | Nothing. The wheel scrolls the transcript. |

The footer names the keys that apply: `esc to cancel` alone, or
`esc to cancel · ctrl+e to explain` where the call sent an explanation and
`esc to cancel · ctrl+e to hide` once it is open. An open explanation cut to
fit adds `↑↓ to see more`. [The long
version](../permissions/permissions.md#the-long-version) is about what
<kbd>Ctrl+E</kbd> shows.

Where there is no room for the panel, or no keyboard, the question is asked a
row at a time and one key answers it: <kbd>y</kbd> or <kbd>Y</kbd> runs the
call once, <kbd>s</kbd> or <kbd>S</kbd> runs it and stops asking for the rest
of the session, and anything else refuses, <kbd>Enter</kbd>, <kbd>Esc</kbd>,
<kbd>Ctrl+C</kbd> and <kbd>Ctrl+D</kbd> among them. Down a pipe a whole line
is read, so `yes` and `session` answer too. See [The three
answers](../permissions/permissions.md#the-three-answers).

## Answering an `ask_user` question

A question the agent puts to you stands in the same place, and [The
keys](../tools/asking.md#the-keys) introduces them. In full:

| Key | What it does |
| --- | --- |
| <kbd>↑</kbd>, <kbd>↓</kbd> | Moves the mark down the answers. |
| <kbd>←</kbd>, <kbd>→</kbd> | Steps to the next question or the one before, and onto the review stop of an ask that has one. |
| <kbd>Enter</kbd> | Takes the marked answer and moves to the next stop, or sends from the last. On `Something else` with nothing written yet, it opens the line instead, and takes it on the next press. On the review stop, `Send` sends and `Cancel` leaves. Where several answers may be chosen, it moves on with what you have chosen and does not choose the marked one. |
| <kbd>Space</kbd> | Where several answers may be chosen, chooses the marked one or unchooses it. |
| <kbd>n</kbd> | Opens a line for a note of your own beside the answer. Not on the review stop. |
| <kbd>1</kbd> to <kbd>9</kbd> | Marks that answer and takes it. Where several may be chosen, it chooses or unchooses it instead. The number one past the last answer is the row under the rule, `Say it in the prompt instead`, and leaves. On the review stop, <kbd>1</kbd> sends and <kbd>2</kbd> cancels. |
| <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd>, <kbd>Ctrl+D</kbd> | Leaves the whole thing unanswered. Not while you are writing a line: then <kbd>Esc</kbd> stops writing, <kbd>Ctrl+C</kbd> clears what you wrote, and <kbd>Ctrl+D</kbd> does nothing. |
| Anything else | Nothing. The wheel scrolls the transcript. |

While you are writing a line, the editing keys edit it, and <kbd>Enter</kbd>
or <kbd>Esc</kbd> stops writing and keeps what you wrote; the footer reads
`esc to stop typing · enter to keep it`. Otherwise the footer is
`esc to cancel · n for a note` for one question that takes one answer. Any
other ask has a review stop and adds `←→ between questions`. A question that
takes several answers adds `space to choose`, and the review stop itself reads
`esc to cancel · ←→ between questions`. [Several questions
at once](../tools/asking.md#several-questions-at-once) is about that stop.

## Pickers and panels

Every panel stood over the box reads the same few keys unless its section
says otherwise: <kbd>↑</kbd> and <kbd>↓</kbd> move the mark, <kbd>Enter</kbd>
takes it, and <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd> or <kbd>Ctrl+D</kbd> leave it
with nothing changed. A ladder such as `/effort` uses <kbd>←</kbd> and
<kbd>→</kbd> in place of the vertical arrows. Where the window has no room for
a panel, it is not stood.

### `/resume`

| Key | What it does |
| --- | --- |
| Typing, paste | Narrows the list to the sessions whose title or branch holds the text, ignoring case. With no match the list says `no session holds "the text"`. |
| <kbd>↑</kbd>, <kbd>↓</kbd> | Walks the list. In a window 70 columns or wider, the preview beside it follows the mark. |
| <kbd>Enter</kbd> | Picks up the marked session. Nothing while nothing matches. |
| <kbd>Esc</kbd> | Clears the search and marks the top. With nothing to clear, leaves: `cancelled, no session picked up`. |
| <kbd>Ctrl+R</kbd> | Opens a rename over the marked session's title. While it is open, typing and paste edit the title, <kbd>Enter</kbd> saves it (an empty one is refused with `a title cannot be empty`), and <kbd>Esc</kbd> closes it and keeps the search. |
| <kbd>Ctrl+C</kbd>, <kbd>Ctrl+D</kbd> | Leaves, rename open or not. |
| Pointer, click | The pointer lights a row. A click marks the row under it, and a click on the marked row picks it up. |
| Wheel | Over the list walks the mark; over the preview scrolls it; anywhere else scrolls the transcript. |

[Switching without restarting](../sessions/sessions.md#switching-without-restarting)
is about what picking one up does.

### `/model`

| Key | What it does |
| --- | --- |
| Typing, paste | Narrows both panes at once, and an edit puts the marks back at the top. <kbd>Home</kbd>, <kbd>End</kbd> and the word keys work on the search line. |
| <kbd>Tab</kbd> | Crosses between the provider pane and the model pane. |
| <kbd>↑</kbd>, <kbd>↓</kbd> | Walks the pane the mark is in. Stepping onto another provider puts the model and the rung back at the top. |
| <kbd>←</kbd>, <kbd>→</kbd> | Steps the rung on the strip. |
| <kbd>Enter</kbd> | Takes the model and the rung together. Nothing while no model matches. |
| <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd>, <kbd>Ctrl+D</kbd> | Leaves, changing nothing. |
| Pointer, click | The pointer lights a row, and a click where nothing was lit lights it. A click on a lit provider narrows the shelf to it; a click on a lit model marks it, and one on the marked model takes it. |
| Wheel | Scrolls the transcript. The shelf is not a window over more than it shows. |

Typed while a turn runs, the shelf stands with an empty rung strip, and
<kbd>Enter</kbd> stands a `Switch model?` panel under the footer
`esc to go back`. `Yes` holds the model for the turn that starts next; `No`,
<kbd>Esc</kbd>, <kbd>Ctrl+C</kbd> or <kbd>Ctrl+D</kbd> go back to the shelf. See
[A command typed while a turn
runs](../getting-started/getting-started.md#a-command-typed-while-a-turn-runs).

### `/effort`

<kbd>←</kbd> and <kbd>→</kbd> step the rung, <kbd>Enter</kbd> confirms it,
and <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd> or <kbd>Ctrl+D</kbd> leave with
`cancelled, no rung taken`.

### `/fast`

<kbd>↑</kbd> and <kbd>↓</kbd> walk `Standard` and `Fast`, <kbd>Enter</kbd>
takes the one marked, and <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd> or
<kbd>Ctrl+D</kbd> leave with `cancelled, the speed is unchanged`. Typed while a
turn runs, the speed taken is asked for once the turn ends.

### `/theme`

<kbd>↑</kbd> and <kbd>↓</kbd> walk the list in view, <kbd>←</kbd> and
<kbd>→</kbd> switch between the interface list and the code list, and
<kbd>Enter</kbd> takes what is marked. The specimen is redrawn from the marks
on every frame, so leaving with <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd> or
<kbd>Ctrl+D</kbd> changes nothing.
[Commands](../getting-started/getting-started.md#commands) describes the two
lists.

### `/sandbox`

<kbd>←</kbd> and <kbd>→</kbd> switch between the Sandbox and Dependencies
tabs, and <kbd>↑</kbd> and <kbd>↓</kbd> walk the one in view. <kbd>Enter</kbd>
on `Enable sandbox` or `Disable sandbox` does that; on any other row it writes
that row into the transcript. Leaving writes `sandbox settings unchanged`.

### `/login`

The first panel and each list take the common keys; the arrows pass over the
`Subscription` and `API key` headings that stand over rows narrowed by words.
Below the first screen <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd> or <kbd>Ctrl+D</kbd>
go back one screen with the mark where it was, and the footer reads `esc to go
back`. On the first screen, and on one that words after `/login` opened
directly, they cancel with `cancelled, nothing signed in`, and the footer reads
`esc to cancel`.

The key box shows one mark per character and never the key. Typed characters
and a paste go in (a paste is trimmed and its control characters dropped),
<kbd>Backspace</kbd> rubs out the last character, <kbd>Enter</kbd> saves the
key and does nothing while the box is empty, and <kbd>Esc</kbd>,
<kbd>Ctrl+C</kbd> or <kbd>Ctrl+D</kbd> leave it as its footer says. The arrows do nothing here: there is no cursor to move. A key past 16 KiB
is refused whole, and silently. A window too short for the box says `the
window has no room for the key box; make it taller and try /login again`.

While an account login waits on the browser, <kbd>Esc</kbd>,
<kbd>Ctrl+C</kbd> or <kbd>Ctrl+D</kbd> stops it and goes back; on a sign-in
that words opened directly it cancels with `cancelled, nothing signed in`.
Pressed as the sign-in is being stored, it waits for the write, up to ten
seconds, and ends signed in where the write went through, or says `! the
sign-in was being stored when it was stopped; /login shows what is stored`. A
sign-in that fails, is refused or expires ends with `! sign-in did not
complete` and what stays stored. Where a code is typed by
hand instead, typing, paste and <kbd>Backspace</kbd> edit it and
<kbd>Enter</kbd> submits it once it holds something. Past 16 KiB the row
under the box says `authorization input is limited to 16 KiB`. [Account login
today](../providers/providers.md#account-login-today) names the accounts.

### `/logout`

A panel headed `Remove stored credential`, with the common keys and the
footer `esc to cancel`.

### `/help`, and a command refused mid-turn

`/help` writes the list of commands into the transcript, and there is nothing
to close; so does `/release-notes`, which is refused while a turn runs. While a
turn runs `/help` stands as a panel instead: any key closes it, and
so does a click on its rows or a wheel notch, and a resize redraws it. A
command that cannot act while a turn runs stands a panel saying so, with
`esc to close` under it; <kbd>Esc</kbd>, <kbd>Enter</kbd>, <kbd>Ctrl+C</kbd>
and <kbd>Ctrl+D</kbd> all close it. A word typed alone that names no command
stands the same panel, saying the word back with the nearest command names, or
pointing at `/help` when none is near, in place of a name and a reason.

### A large session on pickup

The `This session is large` panel offers `Carry on from summary`,
`Carry all of it` and `Stop asking`, under `enter to choose · esc to carry it
whole`. <kbd>Enter</kbd> takes the marked answer and <kbd>Esc</kbd>,
<kbd>Ctrl+C</kbd> or <kbd>Ctrl+D</kbd> carry the session whole. It is never
asked down a pipe. See [Picking up a large
one](../sessions/sessions.md#picking-up-a-large-one).

### Commands left running

<kbd>Ctrl+B</kbd> between turns stands the `Still running` list, under
`esc to close · enter shows it · x stops it`.

| Key | What it does |
| --- | --- |
| <kbd>↑</kbd>, <kbd>↓</kbd> | Moves the mark. |
| <kbd>Enter</kbd> | Shows what the marked command has printed so far. |
| <kbd>x</kbd> | Stops it, with no confirmation. If the stop fails the panel says `Stop failed; x retries`. |
| Click | Marks the row under the pointer, and a click on the marked row shows it. |
| <kbd>Esc</kbd>, <kbd>Ctrl+B</kbd>, <kbd>Ctrl+C</kbd>, <kbd>Ctrl+D</kbd> | Closes the list. |
| Wheel | Scrolls the transcript. |

Inside what one has printed, <kbd>↑</kbd>, <kbd>↓</kbd> and the wheel scroll
the output, <kbd>Esc</kbd>, <kbd>Ctrl+B</kbd> or <kbd>Enter</kbd> go back to
the list, and <kbd>Ctrl+C</kbd> or <kbd>Ctrl+D</kbd> close the whole thing.
[Finding them again](../tools/commands.md#finding-them-again) shows the list.

### Results cut short

<kbd>Ctrl+O</kbd> stands every result the transcript cut, newest first, and a
click on a result's ` (ctrl+o to expand)` offer stands that one. A result no
longer held in memory is read back from the session log when the view reaches
it, one result's worth at a time: a further one the window reaches says
`read back from the session log as the view moves on to it` until the one
above it leaves, as does one the running turn has not written yet, and a step
stops at each such result rather than passing it.
Between
turns the view takes the place of the box; while a turn runs it stands under
the tail. Results cut after it opened are there the next time it is opened.
The footer reads `esc to close`, or `esc to close · ↑↓ to see more` where
there is more.

| Key | What it does |
| --- | --- |
| <kbd>↑</kbd>, <kbd>↓</kbd>, wheel | An arrow moves a row up or down; a wheel notch moves as many rows as it moves the transcript, six unless [`CRUCIBLE_CODE_MOUSE_SCROLL_SPEED`](../configuration/configuration.md#crucible_code_mouse_scroll_speed) says otherwise. At either end, a notch the view cannot use scrolls the transcript. |
| <kbd>Ctrl+O</kbd>, <kbd>Esc</kbd>, <kbd>Ctrl+C</kbd>, <kbd>Ctrl+D</kbd> | Closes it. |
| Anything else | Nothing, <kbd>Enter</kbd> included. |

With one row of room the view gives it to the transcript and closes.

### The queue

<kbd>Ctrl+Q</kbd> while a turn runs stands the prompts waiting behind it.
While the view stands the turn takes none of them, and closing it releases
them all at once.

| Key | What it does |
| --- | --- |
| <kbd>↑</kbd>, <kbd>↓</kbd> | Moves the mark. |
| <kbd>x</kbd> | Takes the marked prompt back into the box, where it can be edited or sent again. When the queue is then empty the view closes with it. |
| <kbd>Esc</kbd>, <kbd>Ctrl+Q</kbd> | Closes it. |
| Anything else | Nothing while it stands, <kbd>Ctrl+C</kbd> included. |
